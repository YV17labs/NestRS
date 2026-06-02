//! [`InMemoryThrottler`] — the fixed-window counter behind [`ThrottlerGuard`].
//!
//! One instance is shared process-wide (provided as global infrastructure by
//! [`ThrottlerModule::for_root`](crate::ThrottlerModule::for_root)); it holds the
//! default limit and the per-key request windows. A Redis-backed store for
//! multi-process deployments is a future addition — the guard would take a trait
//! object then.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::throttle::Throttle;

/// The result of counting one request against a key.
pub struct Decision {
    pub allowed: bool,
    /// When denied, how long until the window resets (for the `Retry-After` header).
    pub retry_after: Duration,
}

struct Window {
    start: Instant,
    count: u32,
}

/// A process-local fixed-window rate limiter.
///
/// Note: keys are not evicted, so an unbounded set of distinct clients grows the
/// map. Acceptable for the in-process default; the future Redis store handles
/// expiry natively.
pub struct InMemoryThrottler {
    default: Throttle,
    trusted_proxies: Vec<IpAddr>,
    windows: Mutex<HashMap<String, Window>>,
}

impl InMemoryThrottler {
    pub fn new(default: Throttle, trusted_proxies: Vec<IpAddr>) -> Self {
        Self {
            default,
            trusted_proxies,
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Proxies whose direct peer address may supply a trusted `X-Forwarded-For` chain.
    pub fn trusted_proxies(&self) -> &[IpAddr] {
        &self.trusted_proxies
    }

    /// The module-wide default limit, used when a route attaches no `#[meta(Throttle)]`.
    pub fn default_limit(&self) -> Throttle {
        self.default
    }

    /// Count one hit for `key` under `limit`. Fixed window: the first hit opens a
    /// window; once `limit` hits land within it, the rest are denied until it
    /// elapses, after which the window resets.
    pub fn hit(&self, key: &str, limit: Throttle) -> Decision {
        let now = Instant::now();
        let mut windows = self.windows.lock();
        let window = windows.entry(key.to_owned()).or_insert(Window {
            start: now,
            count: 0,
        });
        if now.duration_since(window.start) >= limit.window {
            window.start = now;
            window.count = 0;
        }
        window.count += 1;
        if window.count > limit.limit {
            Decision {
                allowed: false,
                retry_after: limit
                    .window
                    .saturating_sub(now.duration_since(window.start)),
            }
        } else {
            Decision {
                allowed: true,
                retry_after: Duration::ZERO,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_the_limit_then_denies_within_the_window() {
        let throttler = InMemoryThrottler::new(Throttle::per_minute(60), Vec::new());
        let limit = Throttle::new(2, Duration::from_secs(60));

        assert!(throttler.hit("k", limit).allowed);
        assert!(throttler.hit("k", limit).allowed);
        let third = throttler.hit("k", limit);
        assert!(!third.allowed, "the third hit exceeds the limit of 2");
        assert!(third.retry_after > Duration::ZERO);

        // A different client has its own window.
        assert!(throttler.hit("other", limit).allowed);
    }

    #[test]
    fn resets_after_the_window_elapses() {
        let throttler = InMemoryThrottler::new(Throttle::per_minute(60), Vec::new());
        let limit = Throttle::new(1, Duration::from_millis(20));

        assert!(throttler.hit("k", limit).allowed);
        assert!(!throttler.hit("k", limit).allowed, "second hit denied");
        std::thread::sleep(Duration::from_millis(30));
        assert!(
            throttler.hit("k", limit).allowed,
            "window reset, hit allowed"
        );
    }
}
