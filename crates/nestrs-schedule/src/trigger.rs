//! When a cron job fires: the trigger kinds and the cron-expression presets.

use std::time::Duration;

/// When a [`CronJobMeta`](crate::CronJobMeta) fires. The `#[cron_job]` decorator
/// picks the variant from its argument (`every` → [`Interval`](Trigger::Interval),
/// `after` → [`Timeout`](Trigger::Timeout), `cron` → [`Cron`](Trigger::Cron)); the
/// [`Scheduler`](crate::Scheduler) reads it to decide how to tick the job.
pub enum Trigger {
    /// Fixed interval — the `@Interval` analog. First run one interval in.
    Interval(Duration),
    /// Run once, this long after boot — the `@Timeout` analog.
    Timeout(Duration),
    /// A cron expression evaluated by `croner` — the `@Cron` analog. `expr` is a
    /// 5/6/7-field pattern; `tz` is an optional IANA timezone name (e.g.
    /// `"Europe/Paris"`), defaulting to UTC when `None`. Both strings are parsed
    /// once at [`Scheduler`](crate::Scheduler) configure time, so a malformed value
    /// fails the boot.
    Cron {
        expr: &'static str,
        tz: Option<&'static str>,
    },
}

/// Cron-expression presets, mirroring NestJS's `CronExpression` enum so a job
/// reads `#[cron_job(cron = CronExpression::EVERY_MINUTE)]`. Each value is a
/// 6-field `croner` pattern (`sec min hour day month weekday`), so every preset
/// fires at a defined second. A test in this crate parses them all.
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

    /// Every preset must be a valid `croner` pattern with a future occurrence —
    /// guards the table against a typo that would only surface at a user's boot.
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
