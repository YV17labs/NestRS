//! Scheduled methods discovered like routes. `#[scheduled]` on a provider's
//! `impl` block orchestrates per-method `#[cron]` / `#[every]` / `#[after]`
//! attributes; each method ships one cron entry sharing the provider's
//! `#[inject]` deps. Importing [`ScheduleModule`] attaches the [`Scheduler`]
//! to the app at boot.
//!
//! Triggers are validated **at compile time** (string literals, and `#[cron]`'s
//! `tz`) or **at boot** (`CronExpression` presets); a bad value fails the boot
//! naming the offending job.
//!
//! Every replica of an app fires every occurrence unless a job says otherwise:
//! `replicas = "one"` on `#[every]` or `#[cron]` fires each occurrence on the one
//! replica whose claim on it succeeds, through the [`OccurrenceLock`] a backend
//! binds — at most once per occurrence, never at least once. A job is its crate,
//! its host struct and its method, so moving its module inside the crate keeps
//! it; `key = "…"` pins it across a rename of the type or the crate.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — Cron and interval registration, and a tick that failed.
pub const TARGET: &str = "nest_rs::schedule";

mod error;
mod inventory;
mod module;
mod occurrence;
mod replicas;
mod scheduler;
mod trigger;
pub mod unit;

pub use error::OccurrenceLockError;
pub use inventory::{CronJobMeta, RunFn, ScheduledMethod};
pub use module::ScheduleModule;
pub use occurrence::{BACKEND_REMEDY, LOCK_TIMEOUT, Occurrence, OccurrenceClaim, OccurrenceLock};
pub use replicas::Replicas;
// The path `#[every]` / `#[cron]` / `#[after]` emit their `JobTransaction` through.
pub use nest_rs_worker;
pub use scheduler::Scheduler;
pub use trigger::{CronExpression, Trigger};

/// Orchestrator on a provider's `impl` block: each `#[every]`, `#[after]` or
/// `#[cron]` method in it is a job the [`Scheduler`] fires.
///
/// ```
/// # use std::time::Duration;
/// use nest_rs_core::injectable;
/// use nest_rs_schedule::{Replicas, ScheduledMethod, Trigger, scheduled};
///
/// #[injectable]
/// #[derive(Default)]
/// pub struct ReportTasks;
///
/// #[scheduled]
/// impl ReportTasks {
///     #[cron("0 0 2 * * *", tz = "Europe/Paris")]
///     async fn nightly(&self) -> anyhow::Result<()> {
///         Ok(())
///     }
///
///     #[every("30s", replicas = "one")]
///     async fn refresh(&self) -> anyhow::Result<()> {
///         Ok(())
///     }
/// }
///
/// let jobs: Vec<&ScheduledMethod> = nest_rs_core::inventory::iter::<ScheduledMethod>()
///     .filter(|job| job.provider == "ReportTasks")
///     .collect();
/// assert!(jobs.iter().any(|job| job.method == "nightly"
///     && job.replicas == Replicas::Each
///     && matches!(job.trigger, Trigger::Cron { expr: "0 0 2 * * *", tz: Some("Europe/Paris") })));
/// assert!(jobs.iter().any(|job| job.method == "refresh"
///     && job.replicas == Replicas::One
///     && matches!(job.trigger, Trigger::Interval(every) if every == Duration::from_secs(30))));
/// ```
pub use nest_rs_schedule_macros::scheduled;
