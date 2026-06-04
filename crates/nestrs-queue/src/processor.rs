//! Consumer side. apalis types stay inside this crate — generated code names
//! only `::nestrs_queue::*`.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use apalis::layers::retry::{RetryLayer, RetryPolicy};
use apalis::layers::ErrorHandlingLayer;
use apalis::prelude::{Data, Monitor, WorkerBuilder, WorkerFactoryFn};
use apalis_redis::RedisStorage;
use async_trait::async_trait;
use nestrs_core::{run_in_job_context, Container, JobContext};
use serde::{de::DeserializeOwned, Serialize};

use crate::connection::QueueConnection;

/// JSON-round-trippable + Clone (retry keeps a copy) + cross-task safe.
pub trait Job: Serialize + DeserializeOwned + Clone + Send + Sync + Unpin + 'static {}
impl<T> Job for T where T: Serialize + DeserializeOwned + Clone + Send + Sync + Unpin + 'static {}

/// A returned `Err` marks the job failed; apalis retries up to the processor's
/// `retries` budget.
#[async_trait]
pub trait Processor: Send + Sync + 'static {
    type Job: Job;

    async fn process(&self, job: Self::Job) -> anyhow::Result<()>;
}

/// Queue analog of `#[injectable]`'s `from_container`, expressed as a trait so
/// [`register_worker`] can build any processor generically.
pub trait FromContainer: Sized {
    fn from_container(container: &Container) -> Self;
}

pub struct ProcessorMeta {
    pub name: &'static str,
    pub queue: &'static str,
    pub concurrency: usize,
    pub retries: usize,
    /// Monomorphic `register_worker::<P>` — lets the transport mount without
    /// naming `P`.
    pub register: fn(Monitor, QueueConnection, Container, &ProcessorMeta) -> Monitor,
}

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
    // apalis 0.7: one worker processes its fetched batch concurrently
    // (FuturesUnordered), so `concurrency` is the Redis source's fetch buffer
    // — the ceiling on in-flight jobs — not a worker count.
    let storage: RedisStorage<P::Job> =
        conn.consumer_storage::<P::Job>(meta.queue, meta.concurrency);
    // Resolve once per worker (static for its lifetime), not per job.
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

/// Runs inside the optional [`JobContext`] seam (bound by a database module's
/// `WorkerDbContext`), so a processor queries through `Repo` with a pool
/// executor installed. Absent, it runs bare.
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

/// Link-time inventory entry submitted by `#[processor]` for each
/// `#[process]`-tagged method. [`crate::QueueWorker`] drains the registry at
/// boot and filters by
/// [`ReachableProviders`](::nestrs_core::ReachableProviders) so a method on a
/// provider not reachable from the app's module tree is silently skipped.
pub struct ProcessMethod {
    pub name: &'static str,
    pub queue: &'static str,
    pub concurrency: usize,
    pub retries: usize,
    pub provider_type_id: fn() -> TypeId,
    pub register: fn(Monitor, QueueConnection, Container, &ProcessorMeta) -> Monitor,
}

::nestrs_core::inventory::collect!(ProcessMethod);

/// Handler signature the `#[process]` macro emits — apalis types are
/// re-exported through this crate so the generated code stays inside the
/// `::nestrs_queue::*` namespace.
pub type MethodHandler<J> = fn(
    J,
    Data<Container>,
    Data<Option<Arc<dyn JobContext>>>,
) -> Pin<Box<dyn Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send>>;

/// Build one apalis worker for a `#[process]` method. Called by the per-method
/// `register` thunk that `#[processor]` emits; takes the method's typed
/// handler so the job type is monomorphized exactly once per `(provider,
/// method)` pair.
pub fn register_method<J>(
    monitor: Monitor,
    conn: QueueConnection,
    container: Container,
    meta: &ProcessorMeta,
    handler: MethodHandler<J>,
) -> Monitor
where
    J: Job,
{
    let storage: RedisStorage<J> = conn.consumer_storage::<J>(meta.queue, meta.concurrency);
    let job_context = container.get_dyn::<dyn JobContext>();
    let worker = WorkerBuilder::new(meta.queue)
        .layer(ErrorHandlingLayer::new())
        .layer(RetryLayer::new(RetryPolicy::retries(meta.retries)))
        .data(container)
        .data(job_context)
        .backend(storage)
        .build_fn(handler);
    monitor.register(worker)
}
