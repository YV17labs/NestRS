use std::time::Duration;

/// `Copy` so static inventory entries can ship a `Trigger` directly without a
/// boxed allocation.
#[derive(Clone, Copy)]
pub enum Trigger {
    /// First run one interval in (matches `@Interval`).
    Interval(Duration),
    Timeout(Duration),
    /// `expr` is a 5/6/7-field croner pattern; `tz` is an optional IANA name
    /// (UTC when `None`). Both parsed at `Scheduler` configure, so a bad value
    /// fails boot.
    Cron {
        expr: &'static str,
        tz: Option<&'static str>,
    },
}

/// 6-field `sec min hour day month weekday` patterns — every preset fires at
/// a defined second.
pub struct CronExpression;

impl CronExpression {
    pub const EVERY_SECOND: &'static str = "* * * * * *";
    pub const EVERY_5_SECONDS: &'static str = "*/5 * * * * *";
    pub const EVERY_10_SECONDS: &'static str = "*/10 * * * * *";
    pub const EVERY_30_SECONDS: &'static str = "*/30 * * * * *";
    pub const EVERY_MINUTE: &'static str = "0 * * * * *";
    pub const EVERY_5_MINUTES: &'static str = "0 */5 * * * *";
    pub const EVERY_10_MINUTES: &'static str = "0 */10 * * * *";
    pub const EVERY_30_MINUTES: &'static str = "0 */30 * * * *";
    pub const EVERY_HOUR: &'static str = "0 0 * * * *";
    pub const EVERY_2_HOURS: &'static str = "0 0 */2 * * *";
    pub const EVERY_3_HOURS: &'static str = "0 0 */3 * * *";
    pub const EVERY_6_HOURS: &'static str = "0 0 */6 * * *";
    pub const EVERY_12_HOURS: &'static str = "0 0 */12 * * *";
    pub const EVERY_DAY_AT_1AM: &'static str = "0 0 1 * * *";
    pub const EVERY_DAY_AT_6AM: &'static str = "0 0 6 * * *";
    pub const EVERY_DAY_AT_NOON: &'static str = "0 0 12 * * *";
    pub const EVERY_DAY_AT_MIDNIGHT: &'static str = "0 0 0 * * *";
    pub const EVERY_WEEKDAY: &'static str = "0 0 0 * * 1-5";
    pub const EVERY_WEEKEND: &'static str = "0 0 0 * * 6,0";
    pub const EVERY_WEEK: &'static str = "0 0 0 * * 0";
    pub const EVERY_1ST_DAY_OF_MONTH_AT_MIDNIGHT: &'static str = "0 0 0 1 * *";
    pub const EVERY_QUARTER: &'static str = "0 0 0 1 */3 *";
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
