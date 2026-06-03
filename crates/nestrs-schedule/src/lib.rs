//! Scheduled jobs discovered like controllers. `Scheduler` is a
//! [`Transport`](nestrs_core::Transport) — it sees the complete container, so a
//! job may inject anything regardless of module import order.
//!
//! `#[cron_job]` takes exactly one of three triggers — `every = "30s"`
//! (interval), `cron = "..."` (5/6/7-field expression, optionally
//! `tz = "..."`; UTC otherwise), or `after = "10s"` (one-shot). Cron strings
//! are validated at compile time; presets/timezones at boot.

mod meta;
mod scheduled;
mod scheduler;
mod trigger;

pub use meta::{CronJobMeta, RunFn};
pub use scheduled::Scheduled;
pub use scheduler::Scheduler;
pub use trigger::{CronExpression, Trigger};

pub use nestrs_schedule_macros::cron_job;

pub use async_trait::async_trait;
