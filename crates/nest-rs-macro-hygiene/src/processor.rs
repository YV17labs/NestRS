//! `#[queue]` + `#[processor]` + `#[process]`, with the layers a job takes on
//! its block and its method: the expansion reaches `nest-rs-core`,
//! `nest-rs-database`, `nest-rs-pipes` and `nest-rs-queue`, so the umbrella's
//! `queue` feature must pull all four.

use nest_rs::core::{injectable, input};
use nest_rs::pipes::Valid;
use nest_rs::queue::{Attempt, Checkpoint, QueueView, processor, queue};

use crate::pipeline::{HygieneUnitAudit, HygieneUnitRefusal};

#[input]
#[derive(Clone)]
pub struct HygieneCommand {
    #[validate(length(min = 1))]
    pub file: String,
}

#[queue(name = "hygiene", job = HygieneCommand)]
pub struct HygieneQueue;

#[queue(name = "hygiene-tuned", job = HygieneCommand)]
pub struct HygieneTunedQueue;

#[queue(name = "hygiene-import", job = HygieneCommand)]
pub struct HygieneImportQueue;

#[queue(name = "hygiene-sync", job = HygieneCommand)]
pub struct HygieneSyncQueue;

#[queue(name = "hygiene-steady", job = HygieneCommand)]
pub struct HygieneSteadyQueue;

#[injectable]
pub struct HygieneProcessor;

/// A value `#[meta]` declares, read by type.
pub struct HygieneLimit(pub u32);

/// `QueueView`'s path through the umbrella.
pub fn processor_of(view: &QueueView) -> &'static str {
    view.processor()
}

#[processor]
#[meta(HygieneLimit(100))]
#[use_interceptors(HygieneUnitAudit)]
#[use_exception_filters(HygieneUnitRefusal)]
impl HygieneProcessor {
    /// `Valid<T>` reaches `nest-rs-pipes`; `transactional = false` names
    /// `TransactionPolicy` through `nest-rs-database`.
    #[process(queue = HygieneQueue, retries = 1, transactional = false)]
    #[meta(HygieneLimit(10))]
    #[use_interceptors(HygieneUnitAudit)]
    #[use_exception_filters(HygieneUnitRefusal)]
    async fn transcode(&self, job: Valid<HygieneCommand>) -> nest_rs::core::anyhow::Result<()> {
        let _ = job.into_inner().file;
        Ok(())
    }

    /// Every tuning key.
    #[process(
        queue = HygieneTunedQueue,
        concurrency = 4,
        throttle(limit = 10, window = "1m"),
        timeout = "30m",
    )]
    async fn sync(&self, job: HygieneCommand) -> nest_rs::core::anyhow::Result<()> {
        let _ = job.file;
        Ok(())
    }

    /// An `Attempt` beside a `Checkpoint<_>`, in either order.
    #[process(queue = HygieneImportQueue, transactional = false)]
    async fn import(
        &self,
        attempt: Attempt,
        job: HygieneCommand,
        progress: Checkpoint<u32>,
    ) -> nest_rs::core::anyhow::Result<()> {
        let _ = (job.file, progress.get(), attempt.job_id(), attempt.number());
        Ok(())
    }

    #[process(queue = HygieneSyncQueue)]
    fn audit(&self, job: HygieneCommand) -> nest_rs::core::anyhow::Result<()> {
        let _ = job.file;
        Ok(())
    }

    #[process(queue = HygieneSteadyQueue, transactional = false)]
    async fn steady(&self, job: HygieneCommand) -> Result<(), crate::never::Never> {
        let _ = job.file;
        Ok(())
    }

    /// A job compiled out takes its handler and registry entry with it.
    #[cfg(any())]
    #[process(queue = crate::does_not_exist::Queue)]
    async fn compiled_out(&self, job: crate::does_not_exist::Job) -> crate::does_not_exist::Answer {
        crate::does_not_exist::run(job)
    }
}
