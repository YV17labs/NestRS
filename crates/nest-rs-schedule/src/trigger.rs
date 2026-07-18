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
    use chrono::Utc;
    use croner::Cron;
    use std::str::FromStr;

    /// Guards the table against a typo that would only surface at a user's boot.
    #[test]
    fn every_preset_parses_and_has_a_next_occurrence() {
        let presets = [
            CronExpression::EVERY_SECOND,
            CronExpression::EVERY_5_SECONDS,
            CronExpression::EVERY_10_SECONDS,
            CronExpression::EVERY_30_SECONDS,
            CronExpression::EVERY_MINUTE,
            CronExpression::EVERY_5_MINUTES,
            CronExpression::EVERY_10_MINUTES,
            CronExpression::EVERY_30_MINUTES,
            CronExpression::EVERY_HOUR,
            CronExpression::EVERY_2_HOURS,
            CronExpression::EVERY_3_HOURS,
            CronExpression::EVERY_6_HOURS,
            CronExpression::EVERY_12_HOURS,
            CronExpression::EVERY_DAY_AT_1AM,
            CronExpression::EVERY_DAY_AT_6AM,
            CronExpression::EVERY_DAY_AT_NOON,
            CronExpression::EVERY_DAY_AT_MIDNIGHT,
            CronExpression::EVERY_WEEKDAY,
            CronExpression::EVERY_WEEKEND,
            CronExpression::EVERY_WEEK,
            CronExpression::EVERY_1ST_DAY_OF_MONTH_AT_MIDNIGHT,
            CronExpression::EVERY_QUARTER,
            CronExpression::EVERY_YEAR,
        ];
        let now = Utc::now();
        for expr in presets {
            let cron = Cron::from_str(expr).unwrap_or_else(|e| panic!("`{expr}` must parse: {e}"));
            cron.find_next_occurrence(&now, false)
                .unwrap_or_else(|e| panic!("`{expr}` must have a next occurrence: {e}"));
        }
    }
}
