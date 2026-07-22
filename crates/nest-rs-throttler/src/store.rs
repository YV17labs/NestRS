//! [`InMemoryThrottler`] — fixed-window counter shared process-wide, plus the
//! [`ThrottlerStore`] trait it implements (the extension seam for alternative
//! backends like a Redis-backed sliding-window store).
//!
//! Each bucket carries its own window: [`InMemoryThrottler::hit`] evicts an
//! entry only once **its own** window has elapsed (never the current caller's),
//! so a hit on a short-window route can't purge a counter opened under a
//! long-window one. A `MAX_KEYS` cap bounds the live set; beyond expiry the map
//! is otherwise unbounded — acceptable for the in-process default, where a
//! future Redis store would handle expiry natively.
//!
//! **Scope is per-process, by design.** The counter lives in this replica's
//! memory, so N replicas of an app give a client up to N× the configured limit
//! on an auth-sensitive endpoint. That is a deliberate trade for the
//! zero-dependency default; deployments that need a global limit implement
//! [`ThrottlerStore`] over a shared store (Redis) and bind that guard instead.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::rate::Throttle;

/// Cap distinct throttle keys to resist unbounded memory growth. Held across
/// the whole store: each of the [`SHARDS`] shards carries `MAX_KEYS / SHARDS`,
/// so sharding buys concurrency without widening the memory bound.
const MAX_KEYS: usize = 10_000;

/// Number of independently locked shards.
///
/// One mutex over one map serialized **every** throttled request in the
/// process, including requests for unrelated keys. Buckets are wholly
/// independent, so they shard by key hash: a hit touches exactly one shard and
/// contends only with hits that hash to the same one. Power of two, so the
/// index is a mask rather than a division.
const SHARDS: usize = 16;

/// Per-shard key cap — see [`MAX_KEYS`].
const MAX_KEYS_PER_SHARD: usize = MAX_KEYS / SHARDS;

/// Amortize the O(n) expiry sweep: run the full-map `retain` only once every
/// `SWEEP_INTERVAL` hits (and always at capacity), so the common per-request
/// path is O(1) under the global mutex rather than an n-key scan (THROT-R1). A
/// key's own window still resets on its next hit, so per-key correctness holds
/// between sweeps — only *other* expired buckets linger briefly, bounded by
/// `MAX_KEYS`.
const SWEEP_INTERVAL: u32 = 128;

/// The outcome of counting one request against a rate limit.
///
/// `#[non_exhaustive]`: construct it through [`Decision::allowed`] /
/// [`Decision::denied`] so a future field (e.g. a `remaining` count) is not a
/// breaking change for out-of-tree [`ThrottlerStore`] implementors.
#[non_exhaustive]
pub struct Decision {
    /// Whether the request is permitted.
    pub allowed: bool,
    /// When denied, time until the window resets (for the `Retry-After` header).
    pub retry_after: Duration,
}

impl Decision {
    /// A permitted request.
    pub fn allowed() -> Self {
        Self {
            allowed: true,
            retry_after: Duration::ZERO,
        }
    }

    /// A denied request; `retry_after` is the time until the window resets
    /// (surfaced to the client as `Retry-After`).
    pub fn denied(retry_after: Duration) -> Self {
        Self {
            allowed: false,
            retry_after,
        }
    }
}

/// Contract a rate-limit backend fulfils so a [`crate::ThrottlerGuard`]-style
/// guard can interrogate it. The in-process [`InMemoryThrottler`] is the
/// default impl; a shared-store implementor (Redis) swaps in via its own
/// module.
///
/// `hit` is **async**: the guard runs inside an already-async `check_http`,
/// so a networked implementor awaits its round-trip directly — no
/// `block_in_place` bridge occupying a runtime worker per rate-limit check,
/// and no panic on a current-thread runtime. The in-memory default resolves
/// immediately.
#[async_trait]
pub trait ThrottlerStore: Send + Sync + 'static {
    /// Count one hit for `key` under `limit`. Returns whether the request is
    /// allowed and, when denied, the `Retry-After` duration.
    async fn hit(&self, key: &str, limit: Throttle) -> Decision;

    /// Default rate limit applied when a route does not pin one via
    /// `#[meta(Throttle::...)]`.
    fn default_limit(&self) -> Throttle;

    /// IPs whose `X-Forwarded-For` is trusted to identify the real client.
    fn trusted_proxies(&self) -> &[IpAddr];
}

struct Window {
    start: Instant,
    count: u32,
    /// The window duration this bucket was opened under. Eviction and reset
    /// compare against **this**, not the current caller's `limit.window`, so a
    /// short-window route can't expire a long-window route's counter.
    window: Duration,
    /// The request cap this bucket was last opened/reset under. Stored so the
    /// eviction pass can tell a *denying* bucket (`count` over the cap) from an
    /// allowed one without the caller's `limit`, and never evict an active
    /// denial (HTTP-S3).
    limit: u32,
}

impl Window {
    /// Whether this bucket is currently refusing requests — over its cap, or
    /// saturated (`u32::MAX` counts as denial even when the cap is `u32::MAX`,
    /// matching [`InMemoryThrottler::hit`]'s fail-closed overload rule).
    fn is_denying(&self) -> bool {
        self.count > self.limit || self.count == u32::MAX
    }
}

/// The in-process default [`ThrottlerStore`] — fixed-window counters in a
/// bounded map. A distributed deployment swaps in a shared-store implementor.
pub struct InMemoryThrottler {
    default: Throttle,
    trusted_proxies: Vec<IpAddr>,
    shards: Box<[Shard]>,
}

/// One independently locked slice of the key space.
struct Shard {
    windows: Mutex<HashMap<String, Window>>,
    /// Hit counter driving this shard's amortized expiry sweep (THROT-R1).
    /// Wraps harmlessly — only its value mod [`SWEEP_INTERVAL`] matters.
    hits: AtomicU32,
}

impl Default for Shard {
    fn default() -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            hits: AtomicU32::new(0),
        }
    }
}

impl InMemoryThrottler {
    /// Build a throttler with the given default limit and trusted-proxy list.
    pub fn new(default: Throttle, trusted_proxies: Vec<IpAddr>) -> Self {
        Self {
            default,
            trusted_proxies,
            shards: (0..SHARDS).map(|_| Shard::default()).collect(),
        }
    }

    /// The shard owning `key`. Hashing here (rather than reusing the map's own
    /// hash) keeps shard choice independent of `HashMap`'s internal seed, so a
    /// key always lands in the same shard for the process's lifetime.
    fn shard(&self, key: &str) -> &Shard {
        use std::hash::{BuildHasher, RandomState};
        use std::sync::LazyLock;

        // One seed per process: shard placement must be stable across hits, and
        // still unpredictable to a caller trying to pile keys onto one shard.
        static SEED: LazyLock<RandomState> = LazyLock::new(RandomState::new);
        let index = SEED.hash_one(key) as usize % SHARDS;
        &self.shards[index]
    }

    /// IPs whose `X-Forwarded-For` is trusted to identify the real client.
    pub fn trusted_proxies(&self) -> &[IpAddr] {
        &self.trusted_proxies
    }

    /// The default rate limit for routes that pin none.
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
        let shard = self.shard(key);
        let mut windows = shard.windows.lock();
        // Amortized expiry sweep (THROT-R1): the full-map `retain` is O(n) under
        // the global mutex, so run it only every `SWEEP_INTERVAL` hits — and
        // always at capacity, so the eviction pass below sees fresh liveness.
        // Between sweeps a bucket still expires against **its own** window on its
        // next hit (the reset below), so per-key correctness holds; only other
        // expired buckets linger briefly, bounded by `MAX_KEYS`.
        let due = shard
            .hits
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(SWEEP_INTERVAL);
        if due || windows.len() >= MAX_KEYS_PER_SHARD {
            windows.retain(|_, window| now.duration_since(window.start) < window.window);
        }
        // At capacity with a new key: make room by evicting the oldest bucket
        // that is NOT actively denying. An over-limit in-window bucket must
        // never be evicted — dropping it resets a live denial, exactly what an
        // attacker minting fresh keys wants (this compounds the X-Forwarded-For
        // keying fix: cheap fresh keys + evictable denials reset every strict
        // counter — HTTP-S3). If every live bucket is denying, refuse the new
        // key fail-closed rather than sacrifice a denial.
        if !windows.contains_key(key) && windows.len() >= MAX_KEYS_PER_SHARD {
            let victim = windows
                .iter()
                .filter(|(_, window)| !window.is_denying())
                .min_by_key(|(_, window)| window.start)
                .map(|(k, _)| k.clone());
            match victim {
                Some(oldest) => {
                    windows.remove(&oldest);
                }
                None => {
                    return Decision::denied(limit.window);
                }
            }
        }
        let window = windows.entry(key.to_owned()).or_insert(Window {
            start: now,
            count: 0,
            window: limit.window,
            limit: limit.limit,
        });
        if now.duration_since(window.start) >= window.window {
            window.start = now;
            window.count = 0;
            // Adopt the current limit's window/cap in case the route's limit
            // changed since this bucket was opened.
            window.window = limit.window;
            window.limit = limit.limit;
        }
        window.count = window.count.saturating_add(1);
        if window.count > limit.limit || window.count == u32::MAX {
            Decision::denied(
                limit
                    .window
                    .saturating_sub(now.duration_since(window.start)),
            )
        } else {
            Decision::allowed()
        }
    }
}

#[async_trait]
impl ThrottlerStore for InMemoryThrottler {
    async fn hit(&self, key: &str, limit: Throttle) -> Decision {
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
            let mut windows = throttler.shard("k").windows.lock();
            windows.insert(
                "k".to_owned(),
                Window {
                    start: Instant::now(),
                    count: u32::MAX - 1,
                    window: Duration::from_secs(60),
                    limit: u32::MAX,
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
            throttler.shard("k").windows.lock().get("k").unwrap().count,
            u32::MAX,
            "saturating_add caps at u32::MAX",
        );
    }

    #[test]
    fn a_short_window_hit_does_not_evict_a_long_window_bucket() {
        // The cross-window eviction bypass: `retain` must expire each bucket
        // against ITS OWN window, not the current caller's. Otherwise a client
        // exhausts a strict long-window limit (e.g. /login 2/min), then pings a
        // lenient short-window route once *its* window elapses — under the old
        // code that hit's short `limit.window` purged EVERY bucket, silently
        // resetting the strict counter.
        let throttler = InMemoryThrottler::new(Throttle::per_minute(60), Vec::new());
        let long = Throttle::new(2, Duration::from_secs(60));
        let short = Throttle::new(100, Duration::from_millis(10));

        // Exhaust the long-window bucket.
        assert!(throttler.hit("long", long).allowed);
        assert!(throttler.hit("long", long).allowed);
        assert!(
            !throttler.hit("long", long).allowed,
            "long-window bucket is now over its limit of 2",
        );

        // Use the short-window bucket, let ITS window elapse, then hit it again.
        // That later hit is the eviction-trigger call: it must purge only the
        // expired short bucket, never the still-live long one.
        assert!(throttler.hit("short", short).allowed);
        std::thread::sleep(Duration::from_millis(20));
        assert!(throttler.hit("short", short).allowed);

        // The long-window bucket survived: still present, still at its exhausted
        // count — so the client stays denied. The bypass is closed.
        {
            let windows = throttler.shard("long").windows.lock();
            let long_bucket = windows
                .get("long")
                .expect("long-window bucket must survive a short-window eviction pass");
            assert_eq!(long_bucket.count, 3, "long-window counter was not reset");
        }
        assert!(
            !throttler.hit("long", long).allowed,
            "long-window limit still enforced after the short-window hit",
        );
    }

    #[test]
    fn eviction_skips_a_denying_bucket_and_removes_an_allowed_one() {
        // HTTP-S3: at MAX_KEYS the eviction pass must preserve a bucket that is
        // actively denying — dropping it resets a live denial (what an attacker
        // minting fresh keys wants). Here the OLDEST bucket is denying; the old
        // oldest-start eviction would have chosen it. It must survive; an
        // allowed bucket is evicted instead.
        let throttler = InMemoryThrottler::new(Throttle::per_minute(60), Vec::new());
        let now = Instant::now();
        // Capacity is per shard, so fill the shard the newcomer will land in.
        let shard = throttler.shard("newcomer");
        {
            let mut windows = shard.windows.lock();
            windows.insert(
                "deny".to_owned(),
                Window {
                    start: now - Duration::from_secs(5), // oldest
                    count: 5,
                    window: Duration::from_secs(60),
                    limit: 1, // count 5 > limit 1 ⇒ denying
                },
            );
            for i in 0..(MAX_KEYS_PER_SHARD - 1) {
                windows.insert(
                    format!("ok{i}"),
                    Window {
                        start: now - Duration::from_secs(1), // newer than "deny"
                        count: 1,
                        window: Duration::from_secs(60),
                        limit: 100, // allowed
                    },
                );
            }
        }

        // A brand-new key at capacity triggers exactly one eviction.
        let _ = throttler.hit("newcomer", Throttle::new(100, Duration::from_secs(60)));

        let windows = shard.windows.lock();
        assert!(
            windows.contains_key("deny"),
            "an over-limit in-window bucket must never be evicted",
        );
        assert_eq!(
            windows.get("deny").unwrap().count,
            5,
            "the denying bucket's counter must not be reset by eviction",
        );
        assert!(
            windows.contains_key("newcomer"),
            "the new key was admitted by evicting an allowed bucket",
        );
    }

    #[test]
    fn a_full_table_of_denying_buckets_refuses_new_keys_fail_closed() {
        // HTTP-S3: when every live bucket is actively denying, a new key must be
        // refused fail-closed rather than evict a denial to make room.
        let throttler = InMemoryThrottler::new(Throttle::per_minute(60), Vec::new());
        let now = Instant::now();
        let shard = throttler.shard("newcomer");
        {
            let mut windows = shard.windows.lock();
            for i in 0..MAX_KEYS_PER_SHARD {
                windows.insert(
                    format!("deny{i}"),
                    Window {
                        start: now,
                        count: 9,
                        window: Duration::from_secs(60),
                        limit: 1, // all denying
                    },
                );
            }
        }

        let decision = throttler.hit("newcomer", Throttle::new(5, Duration::from_secs(30)));
        assert!(
            !decision.allowed,
            "a new key must be refused fail-closed when every bucket is actively denying",
        );

        let windows = shard.windows.lock();
        assert_eq!(
            windows.len(),
            MAX_KEYS_PER_SHARD,
            "the shard stays full — no denial evicted, no newcomer admitted",
        );
        assert!(
            !windows.contains_key("newcomer"),
            "the refused key must not be inserted",
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
