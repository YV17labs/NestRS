//! Counting behaviour of [`InMemoryThrottler`] (`src/store.rs`) keyed by client.

use std::time::Duration;

use nest_rs_throttler::{InMemoryThrottler, Throttle};

#[test]
fn distinct_keys_have_independent_windows() {
    let store = InMemoryThrottler::new();
    let limit = Throttle::new(1, Duration::from_secs(60));

    assert!(store.hit("alice", limit).allowed);
    // Bob hasn't been counted yet — first hit allowed even though Alice is now over.
    assert!(store.hit("bob", limit).allowed);
    // Alice's second hit within the same window is denied.
    assert!(!store.hit("alice", limit).allowed);
}

#[test]
fn retry_after_is_within_the_configured_window_when_denied() {
    let store = InMemoryThrottler::new();
    let limit = Throttle::new(1, Duration::from_secs(30));

    assert!(store.hit("k", limit).allowed);
    let denied = store.hit("k", limit);
    assert!(!denied.allowed);
    assert!(
        denied.retry_after <= Duration::from_secs(30),
        "retry_after must not exceed the window: {:?}",
        denied.retry_after,
    );
}
