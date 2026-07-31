//! The cross-process [`RedisThrottler`] store: an atomic Lua fixed-window
//! against a real Redis. The one thing the in-memory unit tests cannot cover is
//! **one budget shared across `RedisThrottler` instances** (i.e. across app
//! replicas), enforced by a single round-trip with no check-then-act race.

use std::time::Duration;

use nest_rs_redis::RedisThrottler;
use nest_rs_throttler::{Throttle, ThrottlerStore};

use crate::{connect, unique_key};

/// The window script counts hits up to `limit`, then denies with the real
/// remaining TTL as `Retry-After`.
#[tokio::test]
async fn allows_up_to_the_limit_then_denies_with_a_retry_after() {
    // A generous window so the counter can't roll over mid-test.
    let limit = Throttle::new(3, Duration::from_secs(30));
    let store = RedisThrottler::new(connect().await, limit, Vec::new());
    let key = unique_key("cap");

    for n in 1..=3 {
        let decision = store.hit(&key, limit).await;
        assert!(decision.allowed, "hit {n} of 3 must be allowed");
        assert_eq!(decision.retry_after, Duration::ZERO);
    }

    let denied = store.hit(&key, limit).await;
    assert!(!denied.allowed, "the 4th hit must be denied");
    assert!(
        denied.retry_after > Duration::ZERO && denied.retry_after <= limit.window,
        "Retry-After must be the true remaining window, got {:?}",
        denied.retry_after,
    );
}

/// Two distinct client keys never share a budget.
#[tokio::test]
async fn distinct_keys_have_independent_budgets() {
    let limit = Throttle::new(1, Duration::from_secs(30));
    let store = RedisThrottler::new(connect().await, limit, Vec::new());
    let key_a = unique_key("indep-a");
    let key_b = unique_key("indep-b");

    assert!(
        store.hit(&key_a, limit).await.allowed,
        "a: first hit allowed"
    );
    // b is untouched, so its own budget is intact.
    assert!(
        store.hit(&key_b, limit).await.allowed,
        "b: first hit allowed"
    );
    // a is now spent.
    assert!(
        !store.hit(&key_a, limit).await.allowed,
        "a: second hit denied — b's hit must not have spent a's budget",
    );
}

/// The point of the Redis store: the counter lives in Redis, so two separate
/// [`RedisThrottler`] instances (as two app replicas would be) share **one**
/// budget rather than getting `limit` each.
#[tokio::test]
async fn the_budget_is_shared_across_store_instances() {
    let limit = Throttle::new(2, Duration::from_secs(30));
    // Two independent connections → two independent stores, same Redis.
    let replica_a = RedisThrottler::new(connect().await, limit, Vec::new());
    let replica_b = RedisThrottler::new(connect().await, limit, Vec::new());
    let key = unique_key("shared");

    assert!(
        replica_a.hit(&key, limit).await.allowed,
        "replica a: count 1"
    );
    assert!(
        replica_b.hit(&key, limit).await.allowed,
        "replica b: count 2"
    );
    // The third hit — on either replica — is over the shared cap of 2.
    assert!(
        !replica_a.hit(&key, limit).await.allowed,
        "replica a: count 3 must be denied — the two replicas share one budget",
    );
}
