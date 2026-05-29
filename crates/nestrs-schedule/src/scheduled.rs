//! The scheduled-job trait.

use async_trait::async_trait;

/// A job's logic. Implemented on a `#[cron_job]` struct; the
/// [`Scheduler`](crate::Scheduler) builds the struct from the container each time
/// the job fires and calls `run`. A returned `Err` is logged and the schedule
/// continues — one failed run never stops the job.
#[async_trait]
pub trait Scheduled: Send + Sync + 'static {
    async fn run(&self) -> anyhow::Result<()>;
}
