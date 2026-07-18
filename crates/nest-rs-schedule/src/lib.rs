//! Scheduled methods discovered like routes. `#[scheduled]` on a provider's
//! `impl` block orchestrates per-method `#[cron]` / `#[every]` / `#[after]`
//! attributes; each method ships one cron entry sharing the provider's
//! `#[inject]` deps. Importing [`ScheduleModule`] attaches the [`Scheduler`]
//! to the app at boot.
//!
//! Triggers are validated **at compile time** (string literals) or **at
//! boot** (`CronExpression` presets, IANA timezones); a bad value fails the
//! boot naming the offending job.

// Opts OUT of the workspace `unsafe_code = "forbid"` lint (no `[lints]
// workspace = true` in Cargo.toml): its integration test (`tests/end_to_end.rs`)
// needs `unsafe { std::env::set_var }` for setup, and a Cargo `[lints]` forbid
// also covers test targets and can't be overridden. This lib-level forbid keeps
// the production guarantee (the lib itself has no `unsafe`) without breaking the
// integration test.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod inventory;
mod module;
mod scheduler;
mod trigger;

pub use inventory::{CronJobMeta, RunFn, ScheduledMethod};
pub use module::ScheduleModule;
pub use scheduler::Scheduler;
pub use trigger::{CronExpression, Trigger};

pub use nest_rs_schedule_macros::scheduled;
