use std::time::Duration;

/// `Copy` so static inventory entries can ship a `Trigger` directly without a
/// boxed allocation.
#[derive(Clone, Copy)]
pub enum Trigger {
    /// First run one interval in (matches `@Interval`).
    Interval(Duration),
    /// Fire exactly once, this long after boot (matches `@Timeout` / `#[after]`).
    Timeout(Duration),
    /// `expr` is a 5/6/7-field croner pattern; `tz` is an optional IANA name
    /// (UTC when `None`). Both parsed at `Scheduler` configure, so a bad value
    /// fails boot.
    Cron {
        /// The croner pattern to match against wall-clock time.
        expr: &'static str,
        /// IANA timezone the pattern is evaluated in; `None` means UTC.
        tz: Option<&'static str>,
    },
}

/// 6-field `sec min hour day month weekday` patterns — every preset fires at
/// a defined second.
pub struct CronExpression;

impl CronExpression {
    /// At the top of every second.
    pub const EVERY_SECOND: &'static str = "* * * * * *";
    /// At second 0, 5, 10, … of every minute.
    pub const EVERY_5_SECONDS: &'static str = "*/5 * * * * *";
    /// At second 0, 10, 20, … of every minute.
    pub const EVERY_10_SECONDS: &'static str = "*/10 * * * * *";
    /// At second 0 and 30 of every minute.
    pub const EVERY_30_SECONDS: &'static str = "*/30 * * * * *";
    /// At second 0 of every minute.
    pub const EVERY_MINUTE: &'static str = "0 * * * * *";
    /// At minute 0, 5, 10, … on the top of the second.
    pub const EVERY_5_MINUTES: &'static str = "0 */5 * * * *";
    /// At minute 0, 10, 20, ….
    pub const EVERY_10_MINUTES: &'static str = "0 */10 * * * *";
    /// At minute 0 and 30 of every hour.
    pub const EVERY_30_MINUTES: &'static str = "0 */30 * * * *";
    /// At the top of every hour.
    pub const EVERY_HOUR: &'static str = "0 0 * * * *";
    /// At the top of every second hour (00:00, 02:00, …).
    pub const EVERY_2_HOURS: &'static str = "0 0 */2 * * *";
    /// At the top of every third hour (00:00, 03:00, …).
    pub const EVERY_3_HOURS: &'static str = "0 0 */3 * * *";
    /// At the top of every sixth hour (00:00, 06:00, 12:00, 18:00).
    pub const EVERY_6_HOURS: &'static str = "0 0 */6 * * *";
    /// At 00:00 and 12:00.
    pub const EVERY_12_HOURS: &'static str = "0 0 */12 * * *";
    /// Daily at 01:00.
    pub const EVERY_DAY_AT_1AM: &'static str = "0 0 1 * * *";
    /// Daily at 06:00.
    pub const EVERY_DAY_AT_6AM: &'static str = "0 0 6 * * *";
    /// Daily at 12:00.
    pub const EVERY_DAY_AT_NOON: &'static str = "0 0 12 * * *";
    /// Daily at 00:00.
    pub const EVERY_DAY_AT_MIDNIGHT: &'static str = "0 0 0 * * *";
    /// At 00:00 Monday through Friday.
    pub const EVERY_WEEKDAY: &'static str = "0 0 0 * * 1-5";
    /// At 00:00 on Saturday and Sunday.
    pub const EVERY_WEEKEND: &'static str = "0 0 0 * * 6,0";
    /// At 00:00 every Sunday (start of the week).
    pub const EVERY_WEEK: &'static str = "0 0 0 * * 0";
    /// At 00:00 on the first day of every month.
    pub const EVERY_1ST_DAY_OF_MONTH_AT_MIDNIGHT: &'static str = "0 0 0 1 * *";
    /// At 00:00 on the first day of every third month (quarter start).
    pub const EVERY_QUARTER: &'static str = "0 0 0 1 */3 *";
    /// At 00:00 on January 1st.
    pub const EVERY_YEAR: &'static str = "0 0 0 1 1 *";
}

#[cfg(test)]
mod tests {
    use super::CronExpression;
    use chrono::{DateTime, Utc};
    use croner::Cron;
    use std::str::FromStr;

    /// One row per preset: the constant's *name*, so a failure names what a
    /// reader greps for, and the instants it must fire at.
    macro_rules! pinned {
        ($($name:ident => [$($at:literal),+ $(,)?]),+ $(,)?) => {
            &[$((stringify!($name), CronExpression::$name, &[$($at),+] as &[&str])),+]
        };
    }

    /// Every preset, pinned to the instants it actually fires at.
    ///
    /// Asserting that each one *parses* and has *a* next occurrence — all this
    /// suite used to do — cannot see a preset change meaning, and that is what
    /// a cron library's major release moves. Eight deliberately wrong tables
    /// passed the old assertion: `EVERY_WEEKEND` pointing at Monday and
    /// Tuesday, `EVERY_WEEK` at Wednesday, `EVERY_WEEKDAY` at the weekend,
    /// `EVERY_QUARTER` every fourth month.
    ///
    /// The start is fixed and in UTC, so no daylight-saving rule is in play —
    /// DST behaviour belongs to the scheduler, not to this table. Second 45 of
    /// minute 23 of hour 14 aligns with no preset's period, so nothing passes
    /// by landing on a boundary it started from.
    const START: &str = "2026-03-11T14:23:45Z";
    const PINNED: &[(&str, &str, &[&str])] = pinned![
        EVERY_SECOND => ["2026-03-11T14:23:46Z", "2026-03-11T14:23:47Z", "2026-03-11T14:23:48Z"],
        EVERY_5_SECONDS => ["2026-03-11T14:23:50Z", "2026-03-11T14:23:55Z", "2026-03-11T14:24:00Z"],
        EVERY_10_SECONDS => ["2026-03-11T14:23:50Z", "2026-03-11T14:24:00Z", "2026-03-11T14:24:10Z"],
        EVERY_30_SECONDS => ["2026-03-11T14:24:00Z", "2026-03-11T14:24:30Z", "2026-03-11T14:25:00Z"],
        EVERY_MINUTE => ["2026-03-11T14:24:00Z", "2026-03-11T14:25:00Z", "2026-03-11T14:26:00Z"],
        EVERY_5_MINUTES => ["2026-03-11T14:25:00Z", "2026-03-11T14:30:00Z", "2026-03-11T14:35:00Z"],
        EVERY_10_MINUTES => ["2026-03-11T14:30:00Z", "2026-03-11T14:40:00Z", "2026-03-11T14:50:00Z"],
        EVERY_30_MINUTES => ["2026-03-11T14:30:00Z", "2026-03-11T15:00:00Z", "2026-03-11T15:30:00Z"],
        EVERY_HOUR => ["2026-03-11T15:00:00Z", "2026-03-11T16:00:00Z", "2026-03-11T17:00:00Z"],
        EVERY_2_HOURS => ["2026-03-11T16:00:00Z", "2026-03-11T18:00:00Z", "2026-03-11T20:00:00Z"],
        EVERY_3_HOURS => ["2026-03-11T15:00:00Z", "2026-03-11T18:00:00Z", "2026-03-11T21:00:00Z"],
        EVERY_6_HOURS => ["2026-03-11T18:00:00Z", "2026-03-12T00:00:00Z", "2026-03-12T06:00:00Z"],
        EVERY_12_HOURS => ["2026-03-12T00:00:00Z", "2026-03-12T12:00:00Z", "2026-03-13T00:00:00Z"],
        EVERY_DAY_AT_1AM => ["2026-03-12T01:00:00Z", "2026-03-13T01:00:00Z", "2026-03-14T01:00:00Z"],
        EVERY_DAY_AT_6AM => ["2026-03-12T06:00:00Z", "2026-03-13T06:00:00Z", "2026-03-14T06:00:00Z"],
        EVERY_DAY_AT_NOON => ["2026-03-12T12:00:00Z", "2026-03-13T12:00:00Z", "2026-03-14T12:00:00Z"],
        EVERY_DAY_AT_MIDNIGHT => ["2026-03-12T00:00:00Z", "2026-03-13T00:00:00Z", "2026-03-14T00:00:00Z"],
        EVERY_WEEKDAY => ["2026-03-12T00:00:00Z", "2026-03-13T00:00:00Z", "2026-03-16T00:00:00Z"],
        EVERY_WEEKEND => ["2026-03-14T00:00:00Z", "2026-03-15T00:00:00Z", "2026-03-21T00:00:00Z"],
        EVERY_WEEK => ["2026-03-15T00:00:00Z", "2026-03-22T00:00:00Z", "2026-03-29T00:00:00Z"],
        EVERY_1ST_DAY_OF_MONTH_AT_MIDNIGHT => ["2026-04-01T00:00:00Z", "2026-05-01T00:00:00Z", "2026-06-01T00:00:00Z"],
        EVERY_QUARTER => ["2026-04-01T00:00:00Z", "2026-07-01T00:00:00Z", "2026-10-01T00:00:00Z"],
        EVERY_YEAR => ["2027-01-01T00:00:00Z", "2028-01-01T00:00:00Z", "2029-01-01T00:00:00Z"],
    ];

    /// The prose on each constant, checked against what croner actually does.
    #[test]
    fn every_preset_fires_when_its_documentation_says_it_does() {
        let start: DateTime<Utc> = START.parse().expect("the start instant parses");
        let mut moved = Vec::new();
        for (name, expr, expected) in PINNED {
            let cron = Cron::from_str(expr).unwrap_or_else(|e| panic!("`{name}` must parse: {e}"));
            let mut at = start;
            let mut fired = Vec::with_capacity(expected.len());
            for _ in 0..expected.len() {
                at = cron
                    .find_next_occurrence(&at, false)
                    .unwrap_or_else(|e| panic!("`{name}` must have a next occurrence: {e}"));
                fired.push(at.format("%Y-%m-%dT%H:%M:%SZ").to_string());
            }
            if fired != *expected {
                moved.push(format!(
                    "  {name} (`{expr}`)\n    pinned: {expected:?}\n    fired:  {fired:?}"
                ));
            }
        }
        assert!(
            moved.is_empty(),
            "{} preset(s) no longer fire when their documentation says they do:\n{}\n\
             One means that constant or its prose is wrong; several at once \
             means croner's semantics moved, and the answer is upstream's \
             changelog rather than this table.",
            moved.len(),
            moved.join("\n"),
        );
    }

    /// [`PINNED`] says *every* preset, so no constant may be added without a
    /// row — counted off this file, because Rust offers no reflection over
    /// associated constants and a hand-kept count is the drift this suite
    /// exists to catch.
    #[test]
    fn the_pinned_table_covers_every_preset() {
        let declared = include_str!("trigger.rs")
            .lines()
            .filter(|line| line.starts_with("    pub const "))
            .count();
        assert_eq!(
            PINNED.len(),
            declared,
            "`CronExpression` declares {declared} presets and the pinned table \
             holds {}. Add the new preset's row — an unpinned preset is one \
             whose documentation nothing checks.",
            PINNED.len(),
        );
    }
}
