//! [`InMemoryThrottler`] — the in-process fixed-window counter, plus the
//! [`ThrottlerStore`] trait every backend implements.
//!
//! Each bucket expires against **its own** window, never the current caller's,
//! so a short-window route cannot purge a long-window route's counter.
//!
//! **Scope is per-process**: N replicas give a client up to N× the configured
//! limit; a shared store bound beside the module counts across them.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::throttle::Throttle;

/// Cap on distinct throttle keys across the store; each of the [`SHARDS`]
/// shards carries `MAX_KEYS / SHARDS`.
const MAX_KEYS: usize = 10_000;

/// Number of independently locked shards: a hit contends only with hits on its
/// shard. Power of two, so the index is a mask.
const SHARDS: usize = 16;

/// Per-shard key cap — see [`MAX_KEYS`].
const MAX_KEYS_PER_SHARD: usize = MAX_KEYS / SHARDS;

/// Run the O(n) expiry sweep only once every `SWEEP_INTERVAL` hits (and always
/// at capacity); a key's own window still resets on its next hit.
const SWEEP_INTERVAL: u32 = 128;

/// The outcome of counting one request against a rate limit, built through
/// [`Decision::allowed`] / [`Decision::denied`].
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

/// How long [`ThrottlerGuard`](crate::ThrottlerGuard) waits on
/// [`ThrottlerStore::hit`] before it treats the store as one that cannot
/// answer: **20 seconds**.
///
/// **A net under the store, never the store's budget**: past it the guard denies
/// for the whole window, fail closed, and says so at `warn`. It sits above
/// `RedisThrottler`'s per-command budget (`<PREFIX>_REDIS__CONNECT_TIMEOUT_SECS`,
/// 10 s by default), so it never pre-empts a named cause, and below the HTTP
/// request timeout (`<PREFIX>_HTTP__REQUEST_TIMEOUT_SECS`, 30 s), so a hung store
/// reads the same on every edge.
pub const HIT_TIMEOUT: Duration = Duration::from_secs(20);

/// Contract a rate-limit backend fulfils for [`crate::ThrottlerGuard`]. The
/// in-process [`InMemoryThrottler`] is the default; a shared-store implementor
/// swaps in through its own module.
///
/// **`hit` answers within [`HIT_TIMEOUT`], or is treated as unable to**: a
/// networked store bounds each command it sends. A call is dropped where it
/// stands at the bound, so a store keeps no state a dropped call would have had
/// to undo.
#[async_trait]
pub trait ThrottlerStore: Send + Sync + 'static {
    /// Count one hit for `key` under `limit`. Returns whether the request is
    /// allowed and, when denied, the `Retry-After` duration.
    async fn hit(&self, key: &str, limit: Throttle) -> Decision;

    /// The name the guard reports this store under when it does not answer;
    /// defaults to the implementor's type name.
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Whether this store's counters live in this process's memory alone. One
    /// whose counters leave it is handed each bucket's subject as an HMAC under
    /// [`pseudonym_key`](crate::ThrottlerConfig::pseudonym_key), and the boot
    /// refuses it without that key. `false` unless a store says otherwise.
    fn in_process(&self) -> bool {
        false
    }
}

struct Window {
    start: Instant,
    count: u32,
    /// The window this bucket was opened under, which eviction and reset compare
    /// against rather than the current caller's.
    window: Duration,
    /// The cap this bucket was last opened under, so eviction can tell a denying
    /// bucket without the caller's `limit`.
    limit: u32,
}

impl Window {
    /// Whether this bucket is currently refusing requests — over its cap, or
    /// saturated at `u32::MAX`.
    fn is_denying(&self) -> bool {
        self.count > self.limit || self.count == u32::MAX
    }
}

/// The in-process default [`ThrottlerStore`] — fixed-window counters in a
/// bounded map. Which limit applies and **who** a hit belongs to are the port's
/// answers, never a store's.
pub struct InMemoryThrottler {
    shards: Box<[Shard]>,
}

/// One independently locked slice of the key space.
struct Shard {
    windows: Mutex<HashMap<String, Window>>,
    /// Hit counter driving this shard's amortized expiry sweep; only its value mod
    /// [`SWEEP_INTERVAL`] matters.
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

impl Default for InMemoryThrottler {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryThrottler {
    /// An empty store.
    pub fn new() -> Self {
        Self {
            shards: (0..SHARDS).map(|_| Shard::default()).collect(),
        }
    }

    /// The shard owning `key`, hashed independently of `HashMap`'s own seed so a
    /// key stays in one shard.
    fn shard(&self, key: &str) -> &Shard {
        use std::hash::{BuildHasher, RandomState};
        use std::sync::LazyLock;

        // One seed per process: shard placement must be stable across hits, and
        // still unpredictable to a caller trying to pile keys onto one shard.
        static SEED: LazyLock<RandomState> = LazyLock::new(RandomState::new);
        let index = SEED.hash_one(key) as usize % SHARDS;
        &self.shards[index]
    }

    /// Count one hit for `key` under `limit`. Fixed window: the first hit opens
    /// a window; the rest are denied until it elapses.
    ///
    /// **Saturation is denial**: once the counter reaches `u32::MAX` the decision
    /// is `denied` until the window elapses, even under a `u32::MAX` limit.
    pub fn hit(&self, key: &str, limit: Throttle) -> Decision {
        let now = Instant::now();
        let shard = self.shard(key);
        let mut windows = shard.windows.lock();
        let due = shard
            .hits
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(SWEEP_INTERVAL);
        if due || windows.len() >= MAX_KEYS_PER_SHARD {
            windows.retain(|_, window| now.duration_since(window.start) < window.window);
        }
        // At capacity, evict the oldest bucket that is NOT denying: dropping a denial
        // would reset it for an attacker minting fresh keys. If every bucket denies,
        // refuse the new key.
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
                    return Decision::denied(limit.window());
                }
            }
        }
        let window = windows.entry(key.to_owned()).or_insert(Window {
            start: now,
            count: 0,
            window: limit.window(),
            limit: limit.limit(),
        });
        if now.duration_since(window.start) >= window.window {
            window.start = now;
            window.count = 0;
            // Adopt the current limit's window/cap in case the route's limit
            // changed since this bucket was opened.
            window.window = limit.window();
            window.limit = limit.limit();
        }
        window.count = window.count.saturating_add(1);
        if window.count > limit.limit() || window.count == u32::MAX {
            Decision::denied(
                limit
                    .window()
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

    fn in_process(&self) -> bool {
        true
    }
}

/// What the boot tells you when two vendor bindings both bound the store,
/// shared with every store adapter as part of the store contract.
pub const BACKEND_REMEDY: &str = "Import exactly one throttler store binding beside \
                                  `ThrottlerModule::for_root`: `nest_rs::redis::RedisThrottlerModule` \
                                  shares the counters across instances; with none, they stay \
                                  in this process.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_the_limit_then_denies_within_the_window() {
        let throttler = InMemoryThrottler::new();
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
        let throttler = InMemoryThrottler::new();
        let limit = Throttle::new(u32::MAX, Duration::from_secs(60));

        // One shy of saturation, set directly: billions of real hits would dominate
        // the test runtime.
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

        let decision = throttler.hit("k", limit);
        assert!(
            !decision.allowed,
            "saturation must be treated as denial, even when limit == u32::MAX",
        );

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
        let throttler = InMemoryThrottler::new();
        let long = Throttle::new(2, Duration::from_secs(60));
        let short = Throttle::new(100, Duration::from_millis(10));

        assert!(throttler.hit("long", long).allowed);
        assert!(throttler.hit("long", long).allowed);
        assert!(
            !throttler.hit("long", long).allowed,
            "long-window bucket is now over its limit of 2",
        );

        // This later hit triggers the eviction pass.
        assert!(throttler.hit("short", short).allowed);
        std::thread::sleep(Duration::from_millis(20));
        assert!(throttler.hit("short", short).allowed);

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
        // The OLDEST bucket is the denying one.
        let throttler = InMemoryThrottler::new();
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
        let throttler = InMemoryThrottler::new();
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
        let throttler = InMemoryThrottler::new();
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
