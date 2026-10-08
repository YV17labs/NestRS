//! [`Throttle`] — the module-wide default and per-route `#[meta(Throttle::...)]`
//! override.

use std::time::Duration;

/// At most `limit` requests per `window`, per client.
///
/// **Its window is never under a millisecond**, by construction: a zero window
/// resets every bucket on every hit, so every request would be allowed.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Throttle {
    limit: u32,
    window: Duration,
}

/// The port's default rate limit when neither config nor a route pins one:
/// 60/minute. Deliberately **not** `Throttle`'s `Default`, so a guard built
/// outside `ThrottlerModule::for_root` cannot run it silently.
pub const DEFAULT_THROTTLE: Throttle = Throttle::per_minute(60);

impl Throttle {
    /// The shortest window a throttle counts over: the resolution of the
    /// coarsest store the framework ships, Redis's `PEXPIRE`.
    pub const MIN_WINDOW: Duration = Duration::from_millis(1);

    /// `limit` requests per `window`.
    ///
    /// # Panics
    ///
    /// When `window` is under [`MIN_WINDOW`](Self::MIN_WINDOW): a compile error in
    /// a `const`, a failed boot at the route that declares it. A window a
    /// deployment sets is refused by `ThrottlerConfig` instead.
    pub const fn new(limit: u32, window: Duration) -> Self {
        assert!(
            window.as_nanos() >= Self::MIN_WINDOW.as_nanos(),
            "a throttle window under a millisecond limits nothing: every hit opens a new window, \
             so the count never passes one — give `Throttle::new` a window of at least a \
             millisecond",
        );
        Self { limit, window }
    }

    /// Maximum requests permitted per [`window`](Self::window), per client.
    pub const fn limit(&self) -> u32 {
        self.limit
    }

    /// The window the [`limit`](Self::limit) applies over — fixed, opened by
    /// the first request counted in it, in every store this crate and
    /// `nest-rs-redis` ship. Never under [`MIN_WINDOW`](Self::MIN_WINDOW).
    pub const fn window(&self) -> Duration {
        self.window
    }

    /// `limit` requests per minute.
    pub const fn per_minute(limit: u32) -> Self {
        Self::new(limit, Duration::from_secs(60))
    }

    /// `limit` requests per second.
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
        assert_eq!(t.limit(), 120);
        assert_eq!(t.window(), Duration::from_secs(60));
    }

    #[test]
    fn per_second_sets_a_1_second_window() {
        let t = Throttle::per_second(5);
        assert_eq!(t.limit(), 5);
        assert_eq!(t.window(), Duration::from_secs(1));
    }

    #[test]
    #[should_panic(expected = "a throttle window under a millisecond limits nothing")]
    fn a_zero_window_is_refused() {
        let _ = Throttle::new(1, Duration::ZERO);
    }

    #[test]
    #[should_panic(expected = "a throttle window under a millisecond limits nothing")]
    fn a_window_redis_would_count_as_zero_is_refused() {
        let _ = Throttle::new(1, Duration::from_micros(999));
    }

    #[test]
    fn the_shortest_window_is_a_millisecond() {
        assert_eq!(
            Throttle::new(1, Throttle::MIN_WINDOW).window(),
            Duration::from_millis(1)
        );
    }

    #[test]
    fn new_pins_caller_supplied_window() {
        let t = Throttle::new(10, Duration::from_secs(30));
        assert_eq!(t.limit(), 10);
        assert_eq!(t.window(), Duration::from_secs(30));
    }
}
