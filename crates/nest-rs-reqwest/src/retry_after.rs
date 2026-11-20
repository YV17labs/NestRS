//! [`retry_after`] — the wait a `Retry-After` asks, in either form RFC 9110
//! §10.2.3 gives it, read as every framework client reads it.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::delay_seconds::delay_seconds;

const SECONDS_PER_DAY: u64 = 86_400;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

const LONG_DAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// The wait `value` asks: its `delay-seconds`, or its HTTP-date counted from
/// `date`, the same answer's `Date` — the peer's clock read against itself, so
/// no skew between the two hosts enters the wait — or from this host's clock
/// when the answer sent none. A date already past asks no wait; a value in
/// neither form asks nothing.
pub fn retry_after(value: &[u8], date: Option<&[u8]>) -> Option<Duration> {
    if let Some(delay) = delay_seconds(value) {
        return Some(delay);
    }
    let asked = http_date(std::str::from_utf8(value).ok()?, now())?;
    let from = date
        .and_then(|date| std::str::from_utf8(date).ok())
        .and_then(|date| http_date(date, now()))
        .unwrap_or_else(now);
    Some(Duration::from_secs(asked.saturating_sub(from)))
}

/// This host's clock, in whole seconds since the epoch.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// RFC 9110 §5.6.7's HTTP-date in `text`, in seconds since the epoch: the
/// IMF-fixdate it sends, and the obsolete RFC 850 and asctime forms a
/// recipient must still accept; a two-digit year is the latest one, as of
/// `today`, that is not more than fifty years ahead.
fn http_date(text: &str, today: u64) -> Option<u64> {
    let text = text.trim();
    let (day, rest) = text.split_once(", ").unzip();
    let fields: Vec<&str> = match rest {
        Some(rest) => rest.split(' ').collect(),
        None => text.split_whitespace().collect(),
    };
    match (day, fields.as_slice()) {
        (Some(day), [date, month, year, time, "GMT"]) if DAYS.contains(&day) => {
            civil(year.parse().ok()?, month, date, time)
        }
        (Some(day), [date, time, "GMT"]) if LONG_DAYS.contains(&day) => {
            let mut parts = date.split('-');
            let (day_of_month, month, year) = (parts.next()?, parts.next()?, parts.next()?);
            if parts.next().is_some() || year.len() != 2 {
                return None;
            }
            civil(
                two_digit_year(year.parse().ok()?, today),
                month,
                day_of_month,
                time,
            )
        }
        (None, [day, month, date, time, year]) if DAYS.contains(day) => {
            civil(year.parse().ok()?, month, date, time)
        }
        _ => None,
    }
}

/// The year `yy` names, as of `today`: RFC 9110 §5.6.7's rule for RFC 850's
/// two-digit year.
fn two_digit_year(yy: u64, today: u64) -> u64 {
    let this_year = civil_year(today / SECONDS_PER_DAY);
    let mut year = this_year - this_year % 100 + yy;
    if year > this_year + 50 {
        year -= 100;
    }
    year
}

/// The instant `day month year time` names, in seconds since the epoch.
fn civil(year: u64, month: &str, day: &str, time: &str) -> Option<u64> {
    let month = u64::try_from(MONTHS.iter().position(|named| *named == month)? + 1).ok()?;
    let day: u64 = digits(day, 1..=2)?;
    if !(1970..=9999).contains(&year) || day == 0 || day > days_in(year, month) {
        return None;
    }
    let mut clock = time.split(':');
    let (hour, minute, second) = (clock.next()?, clock.next()?, clock.next()?);
    if clock.next().is_some() {
        return None;
    }
    let (hour, minute, second): (u64, u64, u64) = (
        digits(hour, 2..=2)?,
        digits(minute, 2..=2)?,
        digits(second, 2..=2)?,
    );
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some(days * SECONDS_PER_DAY + hour * 3_600 + minute * 60 + second)
}

/// `text` as a number written in `width` ASCII digits.
fn digits(text: &str, width: std::ops::RangeInclusive<usize>) -> Option<u64> {
    (width.contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

fn days_in(year: u64, month: u64) -> u64 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from the epoch to `year-month-day`, from 1970 on: Howard Hinnant's
/// `days_from_civil`.
fn days_from_civil(year: u64, month: u64, day: u64) -> u64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The year the day `days` after the epoch falls in: `civil_from_days`'s.
fn civil_year(days: u64) -> u64 {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    year_of_era + era * 400 + u64::from(month_index >= 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 9110 §5.6.7's example instant, in each of its three forms.
    const SUNDAY: u64 = 784_111_777;

    #[test]
    fn the_three_forms_of_one_instant_read_alike() {
        for form in [
            "Sun, 06 Nov 1994 08:49:37 GMT",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
        ] {
            assert_eq!(http_date(form, SUNDAY), Some(SUNDAY), "{form}");
        }
    }

    #[test]
    fn a_date_is_counted_from_the_answers_own_date_so_no_clock_skew_enters() {
        let asked = retry_after(
            b"Sun, 06 Nov 1994 08:50:07 GMT",
            Some(b"Sun, 06 Nov 1994 08:49:37 GMT".as_slice()),
        );
        assert_eq!(asked, Some(Duration::from_secs(30)));
        let past = retry_after(
            b"Sun, 06 Nov 1994 08:49:00 GMT",
            Some(b"Sun, 06 Nov 1994 08:49:37 GMT".as_slice()),
        );
        assert_eq!(
            past,
            Some(Duration::ZERO),
            "a date already past asks no wait"
        );
    }

    #[test]
    fn delay_seconds_read_as_ever_and_no_date_reads_against_this_hosts_clock() {
        assert_eq!(retry_after(b"120", None), Some(Duration::from_secs(120)));
        assert_eq!(
            retry_after(b"Sun, 06 Nov 1994 08:49:37 GMT", None),
            Some(Duration::ZERO),
            "a date long past, read against this host's clock"
        );
    }

    #[test]
    fn a_two_digit_year_is_the_latest_not_fifty_years_ahead() {
        let in_2026 = days_from_civil(2026, 11, 20) * SECONDS_PER_DAY;
        assert_eq!(two_digit_year(94, in_2026), 1994);
        assert_eq!(two_digit_year(30, in_2026), 2030);
        assert_eq!(
            two_digit_year(76, in_2026),
            2076,
            "fifty years ahead, not more"
        );
        assert_eq!(two_digit_year(77, in_2026), 1977);
        assert_eq!(
            two_digit_year(77, days_from_civil(2020, 1, 1) * SECONDS_PER_DAY),
            1977
        );
    }

    #[test]
    fn what_no_form_writes_asks_nothing() {
        for refused in [
            "Sun, 06 Nov 1994 08:49:37 UTC",
            "Mon, 06 Nov 1994 08:49:37",
            "Sun, 31 Feb 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 24:00:00 GMT",
            "Sun, 06 Xyz 1994 08:49:37 GMT",
            "Sunday, 06-Nov-1994 08:49:37 GMT",
            "1994-11-06T08:49:37Z",
            "-5",
            "",
        ] {
            assert_eq!(retry_after(refused.as_bytes(), None), None, "{refused:?}");
        }
    }

    #[test]
    fn the_civil_arithmetic_holds_at_the_epoch_and_a_leap_day() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 2, 29), 11_016);
        assert_eq!(civil_year(11_016), 2000);
        assert_eq!(civil_year(days_from_civil(2026, 12, 31)), 2026);
        assert_eq!(civil_year(days_from_civil(2027, 1, 1)), 2027);
    }
}
