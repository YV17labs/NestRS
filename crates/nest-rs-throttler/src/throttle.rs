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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_minute_sets_a_60_second_window() {
        let t = Throttle::per_minute(120);
        assert_eq!(t.limit, 120);
        assert_eq!(t.window, Duration::from_secs(60));
    }

    #[test]
    fn per_second_sets_a_1_second_window() {
        let t = Throttle::per_second(5);
        assert_eq!(t.limit, 5);
        assert_eq!(t.window, Duration::from_secs(1));
    }

    #[test]
    fn new_pins_caller_supplied_window() {
        let t = Throttle::new(10, Duration::from_secs(30));
        assert_eq!(t.limit, 10);
        assert_eq!(t.window, Duration::from_secs(30));
    }
}
