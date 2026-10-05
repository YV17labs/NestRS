//! [`RedisThrottler`] — a cross-process rate-limit store backing the
//! `nest-rs-throttler` [`ThrottlerGuard`](nest_rs_throttler::ThrottlerGuard), enabled by the `throttler` feature.
//!
//! Same fixed-window semantics as the in-process
//! [`InMemoryThrottler`](nest_rs_throttler::InMemoryThrottler), but the counter
//! lives in Redis, so N replicas of an app share **one** budget per client
//! instead of N× the limit. The window is advanced by a single atomic Lua
//! script (`INCR` + set-expiry-if-unset + `PTTL`) — one round-trip, no
//! check-then-act race between replicas.
//!
//! **Fail-closed.** [`ThrottlerStore::hit`] is async, so the Redis round-trip
//! is awaited directly on the guard's request task — no worker thread is
//! blocked per rate-limit check. When Redis is unreachable the store **denies**
//! (mirrors the in-memory saturation choice): a rate limiter that fails open
//! under a backend outage is an auth bypass, so the outage is logged at `warn`
//! and the request is refused.

use std::time::Duration;

use async_trait::async_trait;
use nest_rs_throttler::{Decision, Throttle, ThrottlerStore};
use redis::Script;

use crate::RedisConnection;

/// Every key this binding writes: `nestrs:throttler:buckets:<subject>`, one per
/// throttled subject, counting its current window.
///
/// `nestrs:<concern>:<structure>[:<member>]`, like every key the framework
/// writes. The concern is the tail of [`nest_rs_throttler::TARGET`] — the crate
/// that **owns** the concern, never `redis`, because an operator looking at
/// Redis is looking for the rate limiter's keys — so a key names the port that
/// owns it, and the port derives the key. `buckets` is the structure level, read
/// off the port: `InMemoryThrottler` holds one **bucket** per key, and a window is
/// the span a bucket counts in rather than the thing the key holds. Without it
/// the concern had exactly one pattern — itself — so an operator sweeping the
/// rate limiter could scope a `SCAN` no narrower than the concern.
///
/// The prefix is fixed, not the deployment's: `NESTRS_ENV_PREFIX` renames the
/// developer's variables, while a key is the framework's own machinery, and two
/// deployments sharing one Redis are separated by the logical database in the
/// connection URL.
pub(crate) const BUCKETS: &str = "nestrs:throttler:buckets";

/// The key `subject`'s window is counted in. The subject is the port's —
/// `nest_rs_throttler` joins its parts with U+001F, so a route pattern's `:`
/// never reads as a level here.
fn bucket(subject: &str) -> String {
    format!("{BUCKETS}:{subject}")
}

/// Atomic fixed-window step. Returns `{count, ttl_ms}` in one round-trip:
///
/// - `INCR` opens or advances the window counter.
/// - the window's expiry is (re)armed only when the key has none
///   (`PTTL < 0` — a just-created key, or one that somehow lost its TTL), which
///   is the `EXPIRE NX` semantics without a version dependency.
/// - `PTTL` returns the remaining window in ms, so the guard's `Retry-After` is
///   the true time to reset, not a fixed guess.
const WINDOW_SCRIPT: &str = r"
local count = redis.call('INCR', KEYS[1])
local ttl = redis.call('PTTL', KEYS[1])
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
    script: Script,
}

impl RedisThrottler {
    /// `conn` is the app's shared Redis connection (reused, not reopened —
    /// every hit goes over its multiplexed socket, bounded by its budget).
    pub fn new(conn: RedisConnection) -> Self {
        Self {
            conn,
            script: Script::new(WINDOW_SCRIPT),
        }
    }

    /// Run the window script for `key` over the shared connection. `window_ms`
    /// is the current limit's window. Awaited on the guard's own request task —
    /// the [`ThrottlerStore`] seam is async, so no runtime worker is blocked
    /// and a current-thread runtime works too.
    async fn run(&self, key: &str, window_ms: u64) -> Result<(i64, i64), redis::RedisError> {
        self.conn
            .invoke(self.script.key(bucket(key)).arg(window_ms))
            .await
    }
}

#[async_trait]
impl ThrottlerStore for RedisThrottler {
    async fn hit(&self, key: &str, limit: Throttle) -> Decision {
        // Through the crate's one conversion: Redis refuses an expiry whose
        // instant overflows its `i64` milliseconds, which `Throttle::new` lets a
        // window reach — every hit then failed, closed, with a warn apiece.
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

    /// The port's subject follows the structure verbatim, its separators
    /// included.
    #[test]
    fn a_bucket_carries_the_subject_verbatim() {
        let subject = "http\u{1f}/users/:id\u{1f}203.0.113.7";
        assert_eq!(bucket(subject), format!("{BUCKETS}:{subject}"));
    }
}
