//! [`InMemoryThrottler`] — fixed-window counter shared process-wide, plus the
//! [`ThrottlerStore`] trait it implements (the extension seam for alternative
//! backends like a Redis-backed sliding-window store).
//!
//! Keys are not evicted, so an unbounded set of distinct clients grows the map.
//! Acceptable for the in-process default; a future Redis store would handle
//! expiry natively.
//!
//! **Scope is per-process, by design.** The counter lives in this replica's
//! memory, so N replicas of an app give a client up to N× the configured limit
//! on an auth-sensitive endpoint. That is a deliberate trade for the
//! zero-dependency default; deployments that need a global limit implement
//! [`ThrottlerStore`] over a shared store (Redis) and bind that guard instead.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::rate::Throttle;

/// Cap distinct throttle keys to resist unbounded memory growth.
const MAX_KEYS: usize = 10_000;

pub struct Decision {
    pub allowed: bool,
    /// When denied, time until the window resets (for the `Retry-After` header).
    pub retry_after: Duration,
}

/// Contract a rate-limit backend fulfils so a [`crate::ThrottlerGuard`]-style
/// guard can interrogate it. The in-process [`InMemoryThrottler`] is the
/// default impl; a third-party crate (e.g. `nestrs-throttler-redis`) ships an
/// alternative implementor plus its own module + guard that injects the
/// implementor.
///
/// Sync on purpose: a Redis implementor either fronts the call with a
/// `tokio::task::block_in_place` wrapper, or — preferable — wraps a non-async
/// driver (`redis::Commands`) on a dedicated thread pool. Keeping the trait
/// sync lets the existing `Guard::check` flow stay free of an extra await
/// point for the in-memory default.
pub trait ThrottlerStore: Send + Sync + 'static {
    /// Count one hit for `key` under `limit`. Returns whether the request is
    /// allowed and, when denied, the `Retry-After` duration.
    fn hit(&self, key: &str, limit: Throttle) -> Decision;

    /// Default rate limit applied when a route does not pin one via
    /// `#[meta(Throttle::...)]`.
    fn default_limit(&self) -> Throttle;

    /// IPs whose `X-Forwarded-For` is trusted to identify the real client.
    fn trusted_proxies(&self) -> &[IpAddr];
}

struct Window {
    start: Instant,
    count: u32,
}

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

    pub fn trusted_proxies(&self) -> &[IpAddr] {
        &self.trusted_proxies
    }

    pub fn default_limit(&self) -> Throttle {
        self.default
    }

    /// Count one hit for `key` under `limit`. Fixed window: the first hit opens
    /// a window; the rest are denied until it elapses.
    ///
    /// The per-window counter uses `saturating_add` so a flood exceeding
    /// `u32::MAX` requests in one window neither panics in debug nor wraps
    /// to zero in release. **Saturation is treated as denial** (fail-closed
    /// overload defense): once the counter reaches `u32::MAX` the decision
    /// is `denied` until the window elapses, even if `limit.limit` is
    /// itself `u32::MAX`.
    pub fn hit(&self, key: &str, limit: Throttle) -> Decision {
        let now = Instant::now();
        let mut windows = self.windows.lock();
        windows.retain(|_, window| now.duration_since(window.start) < limit.window);
        if !windows.contains_key(key)
            && windows.len() >= MAX_KEYS
            && let Some(oldest) = windows
                .iter()
                .min_by_key(|(_, window)| window.start)
                .map(|(k, _)| k.clone())
        {
            windows.remove(&oldest);
        }
        let window = windows.entry(key.to_owned()).or_insert(Window {
            start: now,
            count: 0,
        });
        if now.duration_since(window.start) >= limit.window {
            window.start = now;
            window.count = 0;
        }
        window.count = window.count.saturating_add(1);
        if window.count > limit.limit || window.count == u32::MAX {
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

impl ThrottlerStore for InMemoryThrottler {
    fn hit(&self, key: &str, limit: Throttle) -> Decision {
        Self::hit(self, key, limit)
    }

    fn default_limit(&self) -> Throttle {
        Self::default_limit(self)
    }

    fn trusted_proxies(&self) -> &[IpAddr] {
        Self::trusted_proxies(self)
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

        assert!(throttler.hit("other", limit).allowed);
    }

    #[test]
    fn count_saturates_without_panicking_and_denies_at_u32_max() {
        // Y2: an unchecked `+= 1` would panic in debug or wrap to 0 in
        // release once the per-window counter passes `u32::MAX` — silently
        // releasing the rate limit. `saturating_add` caps it; saturation
        // is treated as denial (fail-closed overload defense).
        let throttler = InMemoryThrottler::new(Throttle::per_minute(60), Vec::new());
        let limit = Throttle::new(u32::MAX, Duration::from_secs(60));

        // Pre-load the window to one shy of saturation via direct field
        // access — driving it there with billions of real hits would
        // dominate the test runtime.
        {
            let mut windows = throttler.windows.lock();
            windows.insert(
                "k".to_owned(),
                Window {
                    start: Instant::now(),
                    count: u32::MAX - 1,
                },
            );
        }

        // The next hit pushes the count to `u32::MAX` — saturation point.
        // Even though the configured limit is `u32::MAX`, the decision
        // must be `denied` (fail-closed) and must not panic.
        let decision = throttler.hit("k", limit);
        assert!(
            !decision.allowed,
            "saturation must be treated as denial, even when limit == u32::MAX",
        );

        // A further hit stays at `u32::MAX` (saturating) and stays denied
        // — no wrap-to-zero, no panic.
        let next = throttler.hit("k", limit);
        assert!(!next.allowed, "saturated count must remain denied");
        assert_eq!(
            throttler.windows.lock().get("k").unwrap().count,
            u32::MAX,
            "saturating_add caps at u32::MAX",
        );
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
