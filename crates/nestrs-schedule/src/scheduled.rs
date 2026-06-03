use async_trait::async_trait;

/// A returned `Err` is logged and the schedule continues — one failed run
/// never stops the job.
#[async_trait]
pub trait Scheduled: Send + Sync + 'static {
    async fn run(&self) -> anyhow::Result<()>;
}
