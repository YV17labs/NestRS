//! Scheduled methods discovered like routes. `#[scheduled]` on a provider's
//! `impl` block orchestrates per-method `#[cron]` / `#[every]` / `#[after]`
//! attributes; each method ships one cron entry sharing the provider's
//! `#[inject]` deps. A listed host brings [`ScheduleModule`], which attaches
//! the [`Scheduler`] to the app at boot.
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

// The run's unit tests write `#[scheduled]`, which names `::nest_rs_schedule`.
#[cfg(test)]
extern crate self as nest_rs_schedule;

/// This crate's span target — Cron and interval registration, and a tick that failed.
pub const TARGET: &str = "nest_rs::schedule";

mod answer;
mod error;
mod inventory;
mod module;
mod occurrence;
mod replicas;
mod scheduler;
mod trigger;
pub mod unit;
mod view;

pub use error::OccurrenceLockError;
pub use inventory::{CronJobMeta, RunFn, ScheduledMethod};
pub use module::ScheduleModule;
pub use occurrence::{BACKEND_REMEDY, LOCK_TIMEOUT, Occurrence, OccurrenceClaim, OccurrenceLock};
pub use replicas::Replicas;
pub use scheduler::Scheduler;
pub use trigger::{CronExpression, Trigger};
pub use view::ScheduleView;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::answer::{TickAnswer, answered};
    pub use crate::inventory::{ScheduledMethodParts, scheduled_method};
    pub use crate::module::RUNTIME;
    pub use nest_rs_database;
}

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
///     .filter(|job| job.host() == "ReportTasks")
///     .collect();
/// assert!(jobs.iter().any(|job| job.method() == "nightly"
///     && job.replicas() == Replicas::Each
///     && matches!(job.trigger(), Trigger::Cron { expr: "0 0 2 * * *", tz: Some("Europe/Paris") })));
/// assert!(jobs.iter().any(|job| job.method() == "refresh"
///     && job.replicas() == Replicas::One
///     && matches!(job.trigger(), Trigger::Interval(every) if every == Duration::from_secs(30))));
/// ```
///
/// A method answers `anyhow::Result<()>`, or a `Result<(), E>` whose error of
/// its own converts into `anyhow::Error`, which fails the occurrence as `?`
/// would convert it:
///
/// ```
/// # use std::time::Duration;
/// use nest_rs_core::injectable;
/// use nest_rs_schedule::{ScheduledMethod, Trigger, scheduled};
///
/// #[derive(Debug, thiserror::Error)]
/// #[error("the report store refused the connection")]
/// pub struct StoreRefused;
///
/// #[injectable]
/// #[derive(Default)]
/// pub struct ReportTasks;
///
/// #[scheduled]
/// impl ReportTasks {
///     #[after("10s")]
///     async fn warm(&self) -> Result<(), StoreRefused> {
///         Ok(())
///     }
/// }
///
/// let warm = nest_rs_core::inventory::iter::<ScheduledMethod>()
///     .find(|job| job.host() == "ReportTasks" && job.method() == "warm")
///     .expect("the tick is registered");
/// assert!(matches!(warm.trigger(), Trigger::Timeout(after) if after == Duration::from_secs(10)));
/// ```
pub use nest_rs_schedule_macros::scheduled;
