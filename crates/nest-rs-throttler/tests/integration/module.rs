//! The default limit `ThrottlerModule` falls back to (`src/module.rs`).

use std::time::Duration;

use nest_rs_throttler::DEFAULT_THROTTLE;

#[test]
fn default_throttle_constant_is_60_per_minute() {
    // App code reads `ThrottlerConfig.limit.unwrap_or(DEFAULT_THROTTLE.limit)` —
    // a silent change here re-tunes every rate-limited route.
    assert_eq!(DEFAULT_THROTTLE.limit, 60);
    assert_eq!(DEFAULT_THROTTLE.window, Duration::from_secs(60));
}
