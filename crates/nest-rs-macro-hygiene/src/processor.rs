//! `#[queue]` + `#[processor]` + `#[process]` — the queue edge's decorators.
//!
//! Their expansion reaches three crates the developer never names: the job
//! context lives in `nest-rs-worker`, the pipe carriers in `nest-rs-pipes`, and
//! the registry in `nest-rs-queue`. So the umbrella's `queue` feature has to
//! pull `worker` and `pipes` too — this module is what proves it does, rather
//! than a manifest line anyone can read as correct without testing it.

use nest_rs::core::{injectable, input};
use nest_rs::pipes::Valid;
use nest_rs::queue::{Checkpoint, processor, queue};

/// The wire payload, validated by a per-argument pipe below.
#[input]
#[derive(Clone)]
pub struct HygieneCommand {
    #[validate(length(min = 1))]
    pub file: String,
}

/// The port both sides agree on: one queue name, one job type.
#[queue(name = "hygiene", job = HygieneCommand)]
pub struct HygieneQueue;

/// The queue a tuned method drains.
#[queue(name = "hygiene-tuned", job = HygieneCommand)]
pub struct HygieneTunedQueue;

/// The queue a resumable method drains.
#[queue(name = "hygiene-import", job = HygieneCommand)]
pub struct HygieneImportQueue;

/// The queue a synchronous method drains.
#[queue(name = "hygiene-sync", job = HygieneCommand)]
pub struct HygieneSyncQueue;

/// Minimal processor host.
#[queue(name = "hygiene-steady", job = HygieneCommand)]
pub struct HygieneSteadyQueue;

#[injectable]
pub struct HygieneProcessor;

#[processor]
impl HygieneProcessor {
    /// `Valid<T>` is the queue's per-argument pipe form: the wire payload is
    /// `T`, the pipe runs after deserialization, and a rejection becomes a job
    /// error. It is exercised here because the carrier is the part of the
    /// expansion that reaches `nest-rs-pipes`.
    /// `transactional = false` is the opt-out: the expansion names
    /// `JobTransaction` through `nest-rs-worker`, which the queue feature has
    /// to pull for this to resolve.
    #[process(queue = HygieneQueue, retries = 1, transactional = false)]
    async fn transcode(&self, job: Valid<HygieneCommand>) -> nest_rs::core::anyhow::Result<()> {
        let _ = job.into_inner().file;
        Ok(())
    }

    /// Every tuning key: the expansion builds the method's options and its
    /// throttle through `nest-rs-queue` alone.
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

    /// A `Checkpoint<S>` parameter, which the expansion opens through the port
    /// before the body runs.
    #[process(queue = HygieneImportQueue, transactional = false)]
    async fn import(
        &self,
        job: HygieneCommand,
        progress: Checkpoint<u32>,
    ) -> nest_rs::core::anyhow::Result<()> {
        let _ = (job.file, progress.get());
        Ok(())
    }

    /// A synchronous job is called without an `.await`.
    #[process(queue = HygieneSyncQueue)]
    fn audit(&self, job: HygieneCommand) -> nest_rs::core::anyhow::Result<()> {
        let _ = job.file;
        Ok(())
    }

    /// A job compiled out takes its handler and its registry entry with it.
    #[process(queue = HygieneSteadyQueue, transactional = false)]
    async fn steady(&self, job: HygieneCommand) -> Result<(), crate::never::Never> {
        let _ = job.file;
        Ok(())
    }

    #[cfg(any())]
    #[process(queue = crate::does_not_exist::Queue)]
    async fn compiled_out(&self, job: crate::does_not_exist::Job) -> crate::does_not_exist::Answer {
        crate::does_not_exist::run(job)
    }
}
