//! [`QueueKitProcessor`] — one `#[process]` method per case's queue, each
//! declaring what its case proves, all doing what the job's [`Act`] says.

use std::time::Duration;

use nest_rs_core::injectable;
use nest_rs_queue::{Queue, processor};

use super::command::{
    Act, BudgetQueue, ConcurrencyQueue, DeathQueue, DelayQueue, DrainQueue, KitCommand, OnceQueue,
    RenewalQueue, RetryQueue, StallQueue, TakenQueue, TraceQueue,
};
use super::probe;

/// The kit's processor.
#[injectable]
#[derive(Default)]
pub(crate) struct QueueKitProcessor;

#[processor]
impl QueueKitProcessor {
    #[process(queue = OnceQueue)]
    async fn once(&self, job: KitCommand) -> anyhow::Result<()> {
        act(OnceQueue::NAME, job).await
    }

    #[process(queue = ConcurrencyQueue, concurrency = 3)]
    async fn concurrency(&self, job: KitCommand) -> anyhow::Result<()> {
        act(ConcurrencyQueue::NAME, job).await
    }

    #[process(queue = RetryQueue, retries = 1)]
    async fn retry(&self, job: KitCommand) -> anyhow::Result<()> {
        act(RetryQueue::NAME, job).await
    }

    #[process(queue = BudgetQueue, retries = 1)]
    async fn budget(&self, job: KitCommand) -> anyhow::Result<()> {
        act(BudgetQueue::NAME, job).await
    }

    #[process(queue = DeathQueue)]
    async fn death(&self, job: KitCommand) -> anyhow::Result<()> {
        act(DeathQueue::NAME, job).await
    }

    #[process(queue = TakenQueue)]
    async fn taken(&self, job: KitCommand) -> anyhow::Result<()> {
        act(TakenQueue::NAME, job).await
    }

    #[process(queue = StallQueue)]
    async fn stall(&self, job: KitCommand) -> anyhow::Result<()> {
        act(StallQueue::NAME, job).await
    }

    #[process(queue = DrainQueue, concurrency = 2)]
    async fn drain(&self, job: KitCommand) -> anyhow::Result<()> {
        act(DrainQueue::NAME, job).await
    }

    #[process(queue = RenewalQueue)]
    async fn renewal(&self, job: KitCommand) -> anyhow::Result<()> {
        act(RenewalQueue::NAME, job).await
    }

    #[process(queue = DelayQueue)]
    async fn delay(&self, job: KitCommand) -> anyhow::Result<()> {
        act(DelayQueue::NAME, job).await
    }

    #[process(queue = TraceQueue)]
    async fn trace(&self, job: KitCommand) -> anyhow::Result<()> {
        act(TraceQueue::NAME, job).await
    }
}

/// Run one attempt at `job` on `queue` as its [`Act`] says, reporting it.
async fn act(queue: &'static str, job: KitCommand) -> anyhow::Result<()> {
    let (running, attempt) = probe::start(queue, &job);
    let outcome = match job.act {
        Act::Complete => Ok(()),
        Act::Hold { ms } => {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            Ok(())
        }
        Act::FailFirst { attempts } if attempt <= attempts => {
            Err(anyhow::anyhow!("the kit's job fails attempt {attempt}"))
        }
        Act::FailFirst { .. } => Ok(()),
        Act::FailAlways => Err(anyhow::anyhow!("the kit's job fails every attempt")),
        Act::ParkOnce if attempt == 1 => std::future::pending().await,
        Act::ParkOnce => Ok(()),
        Act::ParkAlways => std::future::pending().await,
    };
    running.end(outcome.is_ok());
    outcome
}
