//! [`millis`] — a duration as the whole milliseconds every binding hands Redis:
//! a `PX`, a `PEXPIRE`, a blocking read's wait, a script's argument.

use std::time::Duration;

/// The longest duration sent to Redis: enough to reach the last instant
/// RFC 3339 writes, 9999-12-31T23:59:59.999Z, from the Unix epoch, and short
/// enough that Redis's clock plus it stays below 2^53 milliseconds, an integer
/// a Lua number holds exactly.
pub(crate) const LONGEST: Duration = Duration::from_millis(253_402_300_799_999);

const _: () = assert!(
    LONGEST.as_millis() * 2 < 1 << 53,
    "Redis's clock, up to the same instant, plus the longest duration is a Lua integer",
);

/// `duration` in whole milliseconds, at least one — Redis refuses a zero `PX` —
/// and at most [`LONGEST`]: a duration a declaration may carry — a throttle
/// window of `u64::MAX` ms — would overflow the instant Redis computes from it.
pub(crate) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.min(LONGEST).as_millis())
        .unwrap_or(u64::MAX)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_redis_would_refuse_is_kept_for_the_longest_it_accepts() {
        let longest = u64::try_from(LONGEST.as_millis()).expect("the bound fits");
        assert_eq!(millis(Duration::MAX), longest);
        assert_eq!(millis(Duration::from_millis(u64::MAX)), longest);
        assert_eq!(millis(Duration::from_micros(500)), 1, "never zero");
    }
}
