//! `#[queue]` + `#[processor]` + `#[process]`: the expansion reaches
//! `nest-rs-worker`, `nest-rs-pipes` and `nest-rs-queue`, so the umbrella's
//! `queue` feature must pull all three.

use nest_rs::core::{injectable, input};
use nest_rs::pipes::Valid;
use nest_rs::queue::{Checkpoint, processor, queue};

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

#[processor]
impl HygieneProcessor {
    /// `Valid<T>` reaches `nest-rs-pipes`; `transactional = false` names
    /// `JobTransaction` through `nest-rs-worker`.
    #[process(queue = HygieneQueue, retries = 1, transactional = false)]
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

    #[process(queue = HygieneImportQueue, transactional = false)]
    async fn import(
        &self,
        job: HygieneCommand,
        progress: Checkpoint<u32>,
    ) -> nest_rs::core::anyhow::Result<()> {
        let _ = (job.file, progress.get());
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
