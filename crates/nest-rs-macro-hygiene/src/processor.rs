//! `#[queue]` + `#[processor]` + `#[process]` — the queue edge's decorators.
//!
//! Their expansion reaches three crates the developer never names: the job
//! context lives in `nest-rs-worker`, the pipe carriers in `nest-rs-pipes`, and
//! the registry in `nest-rs-queue`. So the umbrella's `queue` feature has to
//! pull `worker` and `pipes` too — this module is what proves it does, rather
//! than a manifest line anyone can read as correct without testing it.

use nest_rs::core::injectable;
use nest_rs::http::input;
use nest_rs::pipes::Valid;
use nest_rs::queue::{processor, queue};

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

/// Minimal processor host.
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
}
