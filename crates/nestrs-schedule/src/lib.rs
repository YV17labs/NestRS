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

mod meta;
mod scheduled;
mod scheduler;
mod trigger;

pub use meta::{CronJobMeta, RunFn};
pub use scheduled::Scheduled;
pub use scheduler::Scheduler;
pub use trigger::{CronExpression, Trigger};

pub use nestrs_schedule_macros::cron_job;

// Re-exported so a `#[cron_job]` struct can write `#[nestrs_schedule::async_trait]`
// on its `Scheduled` impl without taking a direct `async_trait` dependency.
pub use async_trait::async_trait;
