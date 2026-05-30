//! The consumer side: the [`Processor`] trait an app implements, the
//! [`ProcessorMeta`] that `#[processor]` attaches for discovery, and the
//! `register_worker` thunk that turns a discovered processor into a running
//! apalis worker. All apalis types stay inside this crate — the generated code
//! names only `::nestrs_queue::*`.

use std::sync::Arc;

use apalis::layers::retry::{RetryLayer, RetryPolicy};
use apalis::layers::ErrorHandlingLayer;
use apalis::prelude::{Data, Monitor, WorkerBuilder, WorkerFactoryFn};
use apalis_redis::RedisStorage;
use async_trait::async_trait;
use nestrs_core::{run_in_job_context, Container, JobContext};
use serde::{de::DeserializeOwned, Serialize};

use crate::connection::QueueConnection;

/// The payload bounds apalis's Redis storage imposes on a job: it must
/// round-trip through JSON, cross task boundaries, and be cloneable (the retry
/// layer keeps a copy to re-dispatch a failed attempt).
pub trait Job: Serialize + DeserializeOwned + Clone + Send + Sync + Unpin + 'static {}
impl<T> Job for T where T: Serialize + DeserializeOwned + Clone + Send + Sync + Unpin + 'static {}

/// A queue's consumer logic. Implemented on a `#[processor]` struct; the
/// `QueueWorker` transport builds the struct from the container per job and
/// calls [`process`](Processor::process). A returned `Err` marks the job failed,
/// and apalis retries it up to the processor's `retries` budget.
#[async_trait]
pub trait Processor: Send + Sync + 'static {
    /// The job payload this processor consumes. The producer enqueues the same
    /// type into the matching queue.
    type Job: Job;

    async fn process(&self, job: Self::Job) -> anyhow::Result<()>;
}

/// Build a value from the container. `#[processor]` emits this from the struct's
/// `#[inject]` fields — the queue analog of `#[injectable]`'s `from_container`,
/// expressed as a trait so [`register_worker`] can construct any processor
/// generically.
pub trait FromContainer: Sized {
    fn from_container(container: &Container) -> Self;
}

/// Discovery metadata attached by `#[processor]`. The `QueueWorker` transport
/// reads these via `DiscoveryService::meta::<ProcessorMeta>()` at boot and calls
/// each `register` to mount its apalis worker on the shared [`Monitor`]. Fields
/// are `pub` only so the generated code can build it.
pub struct ProcessorMeta {
    pub name: &'static str,
    pub queue: &'static str,
    pub concurrency: usize,
    pub retries: usize,
    /// Monomorphic `register_worker::<P>`: builds the typed worker for this
    /// processor and registers it on the monitor.
    pub register: fn(Monitor, QueueConnection, Container, &ProcessorMeta) -> Monitor,
}

/// Mount the worker for processor `P` on `monitor`: a Redis-backed source on the
/// queue's namespace, a handler that rebuilds `P` from the container per job, and
/// the configured concurrency and retry budget. `#[processor]` stores this
/// monomorphized as `ProcessorMeta::register`, so the transport never names `P`.
#[doc(hidden)]
pub fn register_worker<P>(
    monitor: Monitor,
    conn: QueueConnection,
    container: Container,
    meta: &ProcessorMeta,
) -> Monitor
where
    P: Processor + FromContainer,
{
    // In 0.7 a single worker processes its fetched batch concurrently (a
    // `FuturesUnordered`), so the concurrency knob is the Redis source's fetch
    // buffer — the ceiling on in-flight jobs — not a worker count.
    let storage: RedisStorage<P::Job> =
        conn.consumer_storage::<P::Job>(meta.queue, meta.concurrency);
    // Resolve the optional ambient-data seam once per worker, not per job — it is
    // static for the worker's lifetime (the scheduler resolves it once too).
    let job_context = container.get_dyn::<dyn JobContext>();
    let worker = WorkerBuilder::new(meta.queue)
        .layer(ErrorHandlingLayer::new())
        .layer(RetryLayer::new(RetryPolicy::retries(meta.retries)))
        .data(container)
        .data(job_context)
        .backend(storage)
        .build_fn(handler::<P>);
    monitor.register(worker)
}

/// The apalis job handler: rebuild the processor from the per-worker container
/// and run it. A processor error becomes a boxed error apalis treats as a failed
/// attempt (and retries per the worker's policy).
///
/// The job runs inside the optional [`JobContext`] seam (bound by a database
/// module's `WorkerDbContext`), so a processor queries through `Repo` with a pool
/// executor installed — no connection injected. Absent, it runs bare.
async fn handler<P>(
    job: P::Job,
    container: Data<Container>,
    job_context: Data<Option<Arc<dyn JobContext>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    P: Processor + FromContainer,
{
    let processor = P::from_container(&container);
    run_in_job_context(job_context.as_ref(), processor.process(job))
        .await
        .map_err(Into::into)
}
