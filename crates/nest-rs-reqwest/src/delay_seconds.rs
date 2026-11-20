//! [`delay_seconds`] — the seconds an `Age`, a `max-age` or a `Retry-After`
//! header says, read as every framework client reads them.

use std::time::Duration;

/// RFC 9111 §1.2.2's `delay-seconds` in `value`: digits alone, a value past
/// what fits read as the largest. An HTTP-date is no delay: a `Retry-After`
/// holding one is read by [`retry_after`](crate::__private::retry_after).
pub fn delay_seconds(value: &[u8]) -> Option<Duration> {
    let digits = std::str::from_utf8(value).ok()?.trim();
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(Duration::from_secs(digits.parse().unwrap_or(u64::MAX)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_digits_are_a_delay_and_an_overflow_is_the_longest() {
        let read = |value: &str| delay_seconds(value.as_bytes());
        assert_eq!(read("120"), Some(Duration::from_secs(120)));
        assert_eq!(read(" 7 "), Some(Duration::from_secs(7)));
        assert_eq!(
            read("99999999999999999999999"),
            Some(Duration::from_secs(u64::MAX))
        );
        for refused in ["Wed, 21 Oct 2015 07:28:00 GMT", "-5", "+5", "1.5", ""] {
            assert_eq!(read(refused), None, "{refused:?}");
        }
        assert_eq!(delay_seconds(&[0xff, b'1']), None, "not text");
    }
}
