//! [`RedisThrottler`] — a cross-process rate-limit store backing the
//! `nest-rs-throttler` [`ThrottlerGuard`], enabled by the `throttler` feature.
//!
//! Same fixed-window semantics as the in-process
//! [`InMemoryThrottler`](nest_rs_throttler::InMemoryThrottler), but the counter
//! lives in Redis, so N replicas of an app share **one** budget per client
//! instead of N× the limit. The window is advanced by a single atomic Lua
//! script (`INCR` + set-expiry-if-unset + `PTTL`) — one round-trip, no
//! check-then-act race between replicas.
//!
//! **Fail-closed.** [`ThrottlerStore::hit`] is sync; a Redis round-trip is not,
//! so the call is bridged with `block_in_place` + `Handle::block_on` exactly as
//! the trait prescribes. When Redis is unreachable the store **denies** (mirrors
//! the in-memory saturation choice): a rate limiter that fails open under a
//! backend outage is an auth bypass, so the outage is logged at `warn` and the
//! request is refused.

use std::net::IpAddr;
use std::time::Duration;

use nest_rs_throttler::{Decision, Throttle, ThrottlerStore};
use redis::Script;
use tokio::runtime::Handle;

use crate::QueueConnection;

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
/// [`RedisThrottlerModule`](crate::RedisThrottlerModule) wire it from config +
/// the shared [`QueueConnection`].
pub struct RedisThrottler {
    conn: QueueConnection,
    default: Throttle,
    trusted_proxies: Vec<IpAddr>,
    script: Script,
}

impl RedisThrottler {
    /// `conn` is the app's shared Redis connection (reused, not reopened —
    /// [`QueueConnection::manager`] hands out the multiplexed handle). `default`
    /// applies to routes that pin no `#[meta(Throttle)]`; `trusted_proxies`
    /// mirrors the in-memory store's `X-Forwarded-For` trust list.
    pub fn new(conn: QueueConnection, default: Throttle, trusted_proxies: Vec<IpAddr>) -> Self {
        Self {
            conn,
            default,
            trusted_proxies,
            script: Script::new(WINDOW_SCRIPT),
        }
    }

    /// Run the window script for `key` over the shared connection. `window_ms`
    /// is the current limit's window. Sync boundary: the [`ThrottlerStore`]
    /// trait is sync (it runs inside `Guard::check`), so the async round-trip is
    /// bridged with `block_in_place` + `Handle::block_on` — requires the guard
    /// to run on a multi-threaded runtime, which the HTTP transport is.
    fn run(&self, key: &str, window_ms: u64) -> Result<(i64, i64), redis::RedisError> {
        // A namespace prefix keeps throttle counters from colliding with queue
        // keys on a shared Redis, and makes them greppable in `redis-cli`.
        let namespaced = format!("nestrs:throttle:{key}");
        let mut conn = self.conn.manager();
        // Borrow the compiled script rather than cloning it: `block_on` drives
        // the invocation to completion before `run` returns, so the borrowed
        // `ScriptInvocation` never needs to outlive `self` — no per-request
        // clone of the Lua body + cached SHA.
        tokio::task::block_in_place(|| {
            Handle::current().block_on(
                self.script
                    .key(namespaced)
                    .arg(window_ms)
                    .invoke_async::<(i64, i64)>(&mut conn),
            )
        })
    }
}

impl ThrottlerStore for RedisThrottler {
    fn hit(&self, key: &str, limit: Throttle) -> Decision {
        // `as u64` is saturating-safe here: a `Duration` window never exceeds
        // `u64::MAX` ms in any real config, and Redis PEXPIRE takes an i64 ms.
        let window_ms = limit.window.as_millis().min(u64::MAX as u128) as u64;
        match self.run(key, window_ms) {
            Ok((count, ttl_ms)) => {
                // Denied when the count has passed the limit — identical rule to
                // the in-memory store (`count > limit.limit`).
                let allowed = count <= i64::from(limit.limit);
                if allowed {
                    Decision {
                        allowed: true,
                        retry_after: Duration::ZERO,
                    }
                } else {
                    // Prefer the real remaining TTL; fall back to the full
                    // window if Redis reported no expiry (defensive).
                    let retry_after = if ttl_ms > 0 {
                        Duration::from_millis(ttl_ms as u64)
                    } else {
                        limit.window
                    };
                    Decision {
                        allowed: false,
                        retry_after,
                    }
                }
            }
            // Fail-closed: a Redis outage must not open the rate limit. Deny and
            // surface the error at `warn` (a security event on the throttler
            // target), asking the client to retry after the window.
            Err(error) => {
                tracing::warn!(
                    target: "nest_rs::throttler",
                    key = %key,
                    error = %error,
                    "redis throttler unavailable; denying (fail-closed)",
                );
                Decision {
                    allowed: false,
                    retry_after: limit.window,
                }
            }
        }
    }

    fn default_limit(&self) -> Throttle {
        self.default
    }

    fn trusted_proxies(&self) -> &[IpAddr] {
        &self.trusted_proxies
    }
}
