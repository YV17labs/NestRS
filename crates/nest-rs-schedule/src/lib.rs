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
// workspace = true` in Cargo.toml): its integration test
// (`tests/integration/end_to_end.rs`)
// needs `unsafe { std::env::set_var }` for setup, and a Cargo `[lints]` forbid
// also covers test targets and can't be overridden. This lib-level forbid keeps
// the production guarantee (the lib itself has no `unsafe`) without breaking the
// integration test.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// This crate's span target — Cron and interval registration, and a tick that failed.
///
/// Declared by the crate that **owns** the concern, which is not always the only
/// crate emitting on it: a sibling and a `*-macros` expansion read this constant
/// rather than spelling a second one, because a target's one job is to say
/// **where** an event came from. A central table in the kernel would have meant
/// `nest-rs-core` holding a name for a concern it does not know exists.
pub const TARGET: &str = "nest_rs::schedule";

mod inventory;
mod module;
mod scheduler;
mod trigger;

pub use inventory::{CronJobMeta, RunFn, ScheduledMethod};
pub use module::ScheduleModule;
// Re-exported so `#[every]` / `#[cron]` / `#[after]` emit their
// `JobTransaction` through this crate's own root, the way every other path the
// decorators name is routed.
pub use nest_rs_worker;
pub use scheduler::Scheduler;
pub use trigger::{CronExpression, Trigger};

pub use nest_rs_schedule_macros::scheduled;
