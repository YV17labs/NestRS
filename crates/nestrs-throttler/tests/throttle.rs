//! Public-API exercise for `Throttle` + `InMemoryThrottler` keyed by client.

use std::net::IpAddr;
use std::time::Duration;

use nestrs_throttler::{DEFAULT_THROTTLE, InMemoryThrottler, Throttle};

#[test]
fn default_throttle_constant_is_60_per_minute() {
    // App code reads `ThrottlerConfig.limit.unwrap_or(DEFAULT_THROTTLE.limit)` —
    // a silent change here re-tunes every rate-limited route.
    assert_eq!(DEFAULT_THROTTLE.limit, 60);
    assert_eq!(DEFAULT_THROTTLE.window, Duration::from_secs(60));
}

#[test]
fn default_limit_is_reported_as_configured() {
    let throttle = Throttle::new(5, Duration::from_secs(10));
    let store = InMemoryThrottler::new(throttle, Vec::new());
    assert_eq!(store.default_limit().limit, 5);
    assert_eq!(store.default_limit().window, Duration::from_secs(10));
}

#[test]
fn trusted_proxies_are_passed_through_unchanged() {
    let ip: IpAddr = "10.0.0.1".parse().expect("ip");
    let store = InMemoryThrottler::new(Throttle::per_second(1), vec![ip]);
    assert_eq!(store.trusted_proxies(), &[ip]);
}

#[test]
fn distinct_keys_have_independent_windows() {
    let store = InMemoryThrottler::new(Throttle::per_second(1), Vec::new());
    let limit = Throttle::new(1, Duration::from_secs(60));

    assert!(store.hit("alice", limit).allowed);
    // Bob hasn't been counted yet — first hit allowed even though Alice is now over.
    assert!(store.hit("bob", limit).allowed);
    // Alice's second hit within the same window is denied.
    assert!(!store.hit("alice", limit).allowed);
}

#[test]
fn retry_after_is_within_the_configured_window_when_denied() {
    let store = InMemoryThrottler::new(Throttle::per_second(1), Vec::new());
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
