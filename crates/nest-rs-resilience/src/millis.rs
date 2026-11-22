//! [`millis`] — a duration as the whole milliseconds this crate's lines file
//! (`open_ms`, `wait_ms`, `retry_after_ms`, `asked_ms`, `ceiling_ms`).

use std::time::Duration;

/// `duration` in whole milliseconds, rounded up as a `Retry-After` is rounded
/// up to whole seconds, so a duration above zero is `1` at least, never the
/// `0` of none; saturating at `u64::MAX`.
pub(crate) fn millis(duration: Duration) -> u64 {
    let rounded =
        duration.as_millis() + u128::from(!duration.subsec_nanos().is_multiple_of(1_000_000));
    u64::try_from(rounded).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_is_its_whole_milliseconds_rounded_up_and_none_is_zero() {
        for (duration, filed) in [
            (Duration::ZERO, 0),
            (Duration::from_nanos(1), 1),
            (Duration::from_micros(500), 1),
            (Duration::from_millis(1), 1),
            (Duration::from_micros(1_500), 2),
            (Duration::from_secs(30), 30_000),
            (Duration::new(57, 1), 57_001),
            (Duration::MAX, u64::MAX),
        ] {
            assert_eq!(millis(duration), filed, "{duration:?}");
        }
    }
}
