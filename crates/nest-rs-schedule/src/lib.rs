//! Scheduled methods discovered like routes. `#[scheduled]` on a provider's
//! `impl` block orchestrates per-method `#[cron]` / `#[every]` / `#[after]`
//! attributes; each method ships one cron entry sharing the provider's
//! `#[inject]` deps. Importing [`ScheduleModule`] attaches the [`Scheduler`]
//! to the app at boot.
//!
//! Triggers are validated **at compile time** (string literals) or **at
//! boot** (`CronExpression` presets, IANA timezones); a bad value fails the
//! boot naming the offending job.

mod inventory;
mod meta;
mod module;
mod scheduler;
mod trigger;

pub use inventory::ScheduledMethod;
pub use meta::{CronJobMeta, RunFn};
pub use module::ScheduleModule;
pub use scheduler::Scheduler;
pub use trigger::{CronExpression, Trigger};

pub use nest_rs_schedule_macros::scheduled;
