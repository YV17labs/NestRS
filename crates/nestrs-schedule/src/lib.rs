//! Scheduled jobs for nestrs, discovered the same way controllers are.
//!
//! A cron job is a struct: `#[cron_job(every = "30s")]` builds it from the
//! container (its `#[inject]` fields), implements [`Scheduled`] for the logic,
//! and emits the single `impl Discoverable` that attaches a [`CronJobMeta`]. The
//! [`Scheduler`] transport reads those metas from the fully-assembled container
//! at `configure` and ticks each job on its period — so there is no central job
//! list and a job is wired by listing it in a `#[module(providers = [...])]`,
//! exactly like a service or controller.
//!
//! Because `Scheduler` is a [`Transport`](nestrs_core::Transport), it receives
//! the complete container after the module tree is built, so a job may inject
//! any provider regardless of module import order.
//!
//! ```ignore
//! #[cron_job(every = "1h")]
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
/// the struct from the container each tick and calls `run`. A returned `Err` is
/// logged and the schedule continues — one failed tick never stops the job.
#[async_trait]
pub trait Scheduled: Send + Sync + 'static {
    async fn run(&self) -> anyhow::Result<()>;
}

/// The thunk `#[cron_job]` generates: build the job from the container and run it
/// once. Borrows the container for the duration of the call.
pub type RunFn =
    for<'a> fn(&'a Container) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

/// Discovery metadata attached by `#[cron_job]`. The [`Scheduler`] reads these
/// via `DiscoveryService::meta::<CronJobMeta>()` at boot and ticks each `run` on
/// its `period`. Fields are `pub` only so the generated code can build it.
pub struct CronJobMeta {
    pub name: &'static str,
    pub period: Duration,
    pub run: RunFn,
}
