//! Scheduled jobs for nestrs, discovered the same way controllers are.
//!
//! A scheduled job is a struct: a `#[cron_job(...)]` decorator builds it from the
//! container (its `#[inject]` fields), implements [`Scheduled`] for the logic, and
//! emits the single `impl Discoverable` that attaches a [`CronJobMeta`]. The
//! [`Scheduler`] transport reads those metas from the fully-assembled container at
//! `configure` and runs each on its [`Trigger`] — so there is no central job list
//! and a job is wired by listing it in a `#[module(providers = [...])]`, exactly
//! like a service or controller.
//!
//! # Three triggers, mirroring `@nestjs/schedule`
//!
//! `#[cron_job]` takes exactly one of three mutually-exclusive arguments:
//!
//! - `every = "30s"` — a fixed interval (NestJS's `@Interval`). Suffixes `ms` /
//!   `s` / `m` / `h`. The first run is one interval after boot, then every
//!   interval.
//! - `cron = "..."` — a cron expression (NestJS's `@Cron`). 5, 6, or 7 fields
//!   (seconds optional), e.g. `"0 */5 * * * *"`. Use a [`CronExpression`] preset
//!   (`CronExpression::EVERY_MINUTE`) for the common cases. Add `tz =
//!   "Europe/Paris"` to evaluate the expression in a specific IANA timezone;
//!   without it the schedule is computed in **UTC** (predictable across hosts).
//! - `after = "10s"` — run **once**, that long after boot (NestJS's `@Timeout`).
//!
//! Because `Scheduler` is a [`Transport`](nestrs_core::Transport), it receives the
//! complete container after the module tree is built, so a job may inject any
//! provider regardless of module import order.
//!
//! ```ignore
//! #[cron_job(cron = CronExpression::EVERY_HOUR)]
//! pub struct PruneSessions {
//!     #[inject] sessions: std::sync::Arc<SessionStore>,
//! }
//!
//! #[nestrs_schedule::async_trait]
//! impl nestrs_schedule::Scheduled for PruneSessions {
//!     async fn run(&self) -> anyhow::Result<()> {
//!         self.sessions.prune_expired().await
//!     }
//! }
//!
//! // main.rs
//! App::new::<AppModule>()?
//!     .transport(Scheduler::new())
//!     .transport(HttpTransport::new())
//!     .run().await
//! ```

mod scheduler;

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use nestrs_core::Container;

pub use scheduler::Scheduler;

pub use nestrs_schedule_macros::cron_job;

// Re-exported so a `#[cron_job]` struct can write `#[nestrs_schedule::async_trait]`
// on its `Scheduled` impl without taking a direct `async_trait` dependency.
pub use async_trait::async_trait;

/// A job's logic. Implemented on a `#[cron_job]` struct; the [`Scheduler`] builds
/// the struct from the container each time the job fires and calls `run`. A
/// returned `Err` is logged and the schedule continues — one failed run never
/// stops the job.
#[async_trait]
pub trait Scheduled: Send + Sync + 'static {
    async fn run(&self) -> anyhow::Result<()>;
}

/// When a [`CronJobMeta`] fires. The `#[cron_job]` decorator picks the variant
/// from its argument (`every` → [`Interval`](Trigger::Interval), `after` →
/// [`Timeout`](Trigger::Timeout), `cron` → [`Cron`](Trigger::Cron)); the
/// [`Scheduler`] reads it to decide how to tick the job.
pub enum Trigger {
    /// Fixed interval — the `@Interval` analog. First run one interval in.
    Interval(Duration),
    /// Run once, this long after boot — the `@Timeout` analog.
    Timeout(Duration),
    /// A cron expression evaluated by `croner` — the `@Cron` analog. `expr` is a
    /// 5/6/7-field pattern; `tz` is an optional IANA timezone name (e.g.
    /// `"Europe/Paris"`), defaulting to UTC when `None`. Both strings are parsed
    /// once at [`Scheduler`] configure time, so a malformed value fails the boot.
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

/// The thunk `#[cron_job]` generates: build the job from the container and run it
/// once. Borrows the container for the duration of the call.
pub type RunFn =
    for<'a> fn(&'a Container) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

/// Discovery metadata attached by `#[cron_job]`. The [`Scheduler`] reads these via
/// `DiscoveryService::meta::<CronJobMeta>()` at boot and runs each `run` on its
/// [`Trigger`]. Fields are `pub` only so the generated code can build it.
pub struct CronJobMeta {
    pub name: &'static str,
    pub trigger: Trigger,
    pub run: RunFn,
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
