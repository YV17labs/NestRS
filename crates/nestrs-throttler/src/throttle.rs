//! [`Throttle`] — the module-wide default and per-route `#[meta(Throttle::...)]`
//! override.

use std::time::Duration;

/// At most `limit` requests per `window`, per client.
#[derive(Clone, Copy, Debug)]
pub struct Throttle {
    pub limit: u32,
    pub window: Duration,
}

impl Throttle {
    pub const fn new(limit: u32, window: Duration) -> Self {
        Self { limit, window }
    }

    pub const fn per_minute(limit: u32) -> Self {
        Self::new(limit, Duration::from_secs(60))
    }

    pub const fn per_second(limit: u32) -> Self {
        Self::new(limit, Duration::from_secs(1))
    }
}
