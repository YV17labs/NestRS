//! [`RedisThrottler`] — a cross-process rate-limit store backing the
//! `nest-rs-throttler` [`ThrottlerGuard`](nest_rs_throttler::ThrottlerGuard), enabled by the `throttler` feature.
//!
//! Same fixed-window semantics as the in-process
//! [`InMemoryThrottler`](nest_rs_throttler::InMemoryThrottler), but the counter
//! lives in Redis, so N replicas of an app share **one** budget per client,
//! advanced by one atomic Lua script.
//!
//! **Fail-closed.** When Redis is unreachable the store **denies**, logged at
//! `warn`: a rate limiter that fails open under an outage is an auth bypass.

use std::time::Duration;

use crate::RedisConnection;
use crate::script::RedisScript;
use async_trait::async_trait;
use nest_rs_throttler::{Decision, Throttle, ThrottlerStore};

/// Every key this binding writes: `nestrs:throttler:buckets:<subject>`, one per
/// throttled subject, counting its current window. The concern is the tail of
/// [`nest_rs_throttler::TARGET`], never `redis`.
pub(crate) const BUCKETS: &str = "nestrs:throttler:buckets";

/// The key `subject`'s window is counted in. The subject is the port's: this
/// store keeps its counters outside the process, so it is handed the subject's
/// pseudonym — hex digits, never the client's address or identity — and a `:`
/// never reaches it.
fn bucket(subject: &str) -> String {
    format!("{BUCKETS}:{subject}")
}

/// Atomic fixed-window step. Returns `{count, ttl_ms}` in one round-trip:
///
/// - `PTTL` is the remaining window in ms, so the guard's `Retry-After` is the
///   true time to reset, not a fixed guess; `INCR` keeps it.
/// - a window in its last millisecond (`PTTL` `0`) has ended, as the
///   in-memory store's does at `start + window`, so `PEXPIRE … 0` deletes it
///   and this hit opens the next one.
/// - `INCR` opens or advances the window counter.
/// - the window's expiry is armed only when the key has none (`PTTL < 0` — a
///   key this hit created, or one that somehow lost its TTL), which is the
///   `EXPIRE NX` semantics without a version dependency.
const WINDOW_SCRIPT: &str = "local ttl = redis.call('PTTL', KEYS[1])
if ttl == 0 then
  redis.call('PEXPIRE', KEYS[1], 0)
  ttl = -2
end
local count = redis.call('INCR', KEYS[1])
if ttl < 0 then
  redis.call('PEXPIRE', KEYS[1], ARGV[1])
  ttl = tonumber(ARGV[1])
end
return {count, ttl}
";

/// Redis-backed [`ThrottlerStore`]. Construct via [`RedisThrottler::new`] or let
/// [`RedisThrottlerModule`](crate::RedisThrottlerModule) wire it over the shared
/// [`RedisConnection`]. It counts and nothing else — which limit applies is the
/// port's policy, carried by the guard.
pub struct RedisThrottler {
    conn: RedisConnection,
    script: RedisScript,
}

impl RedisThrottler {
    /// `conn` is the app's shared Redis connection (reused, not reopened —
    /// every hit goes over its multiplexed socket, bounded by its budget).
    pub fn new(conn: RedisConnection) -> Self {
        Self {
            conn,
            script: RedisScript::new(WINDOW_SCRIPT),
        }
    }

    /// Run the window script for `key` over the shared connection. `window_ms`
    /// is the current limit's window.
    async fn run(&self, key: &str, window_ms: u64) -> Result<(i64, i64), redis::RedisError> {
        self.conn
            .invoke(self.script.keys(&[&bucket(key)]).arg(window_ms))
            .await
    }
}

#[async_trait]
impl ThrottlerStore for RedisThrottler {
    async fn hit(&self, key: &str, limit: Throttle) -> Decision {
        // Redis refuses an expiry whose instant overflows its `i64` milliseconds,
        // which `Throttle::new` lets a window reach.
        let window_ms = crate::layout::millis(limit.window());
        match self.run(key, window_ms).await {
            Ok((count, ttl_ms)) => {
                // Denied when the count has passed the limit — identical rule to
                // the in-memory store (`count > limit.limit()`).
                let allowed = count <= i64::from(limit.limit());
                if allowed {
                    Decision::allowed()
                } else {
                    // Prefer the real remaining TTL; fall back to the full
                    // window if Redis reported no expiry (defensive).
                    let retry_after = if ttl_ms > 0 {
                        Duration::from_millis(ttl_ms as u64)
                    } else {
                        limit.window()
                    };
                    Decision::denied(retry_after)
                }
            }
            // Fail-closed: a Redis outage must not open the rate limit. Deny and
            // surface the error at `warn` (a security event on the throttler
            // target), asking the client to retry after the window.
            Err(error) => {
                tracing::warn!(
                    target: nest_rs_throttler::TARGET,
                    key = %key,
                    error = %nest_rs_core::error_message(&error),
                    "redis throttler unavailable; denying (fail-closed)",
                );
                Decision::denied(limit.window())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The port's subject — a pseudonym — follows the structure verbatim.
    #[test]
    fn a_bucket_carries_the_subject_verbatim() {
        let subject = "3f9c0e7a51d24b8896c1f0aa7d42e913";
        assert_eq!(bucket(subject), format!("{BUCKETS}:{subject}"));
    }
}
