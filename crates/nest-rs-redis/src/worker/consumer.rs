//! apalis-redis `JobConsumer` exposed as a `Transport`: one apalis worker per
//! discovered `#[process]` method on a shared [`Monitor`].
//!
//! **This file is the transport and nothing else.** What a job attempt *is* —
//! the envelope, the trace, the `queue.job` span, the panic catch, the outcome
//! classes, the events and the operation line — is the port's
//! (`nest_rs_queue::consume::attempt`), and discovery is the port's too
//! (`consume::discover`). What stays here is what apalis alone knows: the
//! storage handle, the fetch loop, the retry budget, `concurrency(1)`, the
//! drain at shutdown, and the translation of an [`Attempt`] into apalis's
//! `Abort` / `Failed`.
//!
//! Every queue is consumed as `RedisStorage<serde_json::Value>` — the
//! backend-agnostic wire format.
//!
//! **One job at a time per `#[process]` method.** That is the whole contract,
//! and it is deliberately not configurable: nestrs targets the container, so
//! throughput comes from running more replicas of the worker — the unit the
//! platform already schedules, meters and restarts. A per-method ceiling would
//! be a second, in-process scheduler competing with the first, and the number
//! that makes it correct depends on the pod's CPU share rather than on anything
//! the code can know. Serialized-per-method is the behaviour a reader can
//! predict from the source, and it makes every replica's load identical.
//!
//! **Delivery is exclusive but at-least-once.** Two replicas never receive the
//! same job from the queue (`get_jobs.lua` claims ids in one atomic EVAL), yet a
//! replica's *startup* requeues work its peers are running: apalis-redis calls
//! `reenqueue_orphaned` with a cutoff of `Utc::now()`, which matches every
//! registered consumer rather than only this worker's previous incarnation. A
//! scale-up therefore re-runs in-flight jobs. Both halves are measured in
//! `tests/e2e/replicas.rs`; a `#[process]` handler must be idempotent.

use anyhow::{Context, Result};
use apalis::layers::ErrorHandlingLayer;
use apalis::layers::WorkerBuilderExt;
use apalis::layers::catch_panic::CatchPanicLayer;
use apalis::layers::retry::{RetryLayer, RetryPolicy};
use apalis::prelude::{
    Attempt as ApalisAttempt, Data, Monitor, TaskId, WorkerBuilder, WorkerFactoryFn,
};
use apalis_redis::{Config, RedisStorage};
use async_trait::async_trait;
use nest_rs_core::{Container, Transport};
use nest_rs_queue::ProcessMethod;
use nest_rs_queue::consume::{self, Attempt};
use tokio_util::sync::CancellationToken;

use crate::RedisConnection;
use crate::connection::CONNECTION_REMEDY;

/// The consumer-side transport: drains the `#[processor]` inventory and runs
/// each job's process method against the Redis queue. Attached by
/// [`RedisWorkerModule`](crate::RedisWorkerModule).
pub struct RedisWorker {
    methods: Vec<&'static ProcessMethod>,
    container: Option<Container>,
}

impl RedisWorker {
    /// An empty worker; process methods and the container are wired at boot.
    pub fn new() -> Self {
        Self {
            methods: Vec::new(),
            container: None,
        }
    }
}

impl Default for RedisWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for RedisWorker {
    async fn configure(&mut self, container: &Container) -> Result<()> {
        // Which `#[process]` methods this app serves is the port's answer —
        // module-gated, duplicate-checked, announced — not this backend's.
        self.methods = consume::discover(container)?;

        // Fail fast at boot if methods exist but no connection is seeded.
        if !self.methods.is_empty() {
            container.get::<RedisConnection>().with_context(|| {
                format!("RedisWorker found #[processor]s but {CONNECTION_REMEDY}")
            })?;
        }

        self.container = Some(container.clone());
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()> {
        // No methods: idle until shutdown so this transport doesn't race
        // the app down when it is the only one attached.
        if self.methods.is_empty() {
            cancel.cancelled().await;
            return Ok(());
        }

        let container = self
            .container
            .expect("RedisWorker::configure must run before serve");
        let connection = container
            .get::<RedisConnection>()
            .expect("RedisConnection presence is verified in configure");

        let mut monitor = Monitor::new();
        for method in &self.methods {
            monitor = build_worker(monitor, &connection, container.clone(), method);
        }

        // Bound the post-signal drain so a hung `#[process]` can't block SIGTERM
        // until the orchestrator SIGKILLs the pod (QUEUE-I5). The config is a
        // factory output `RedisWorkerModule::for_root` resolved.
        let shutdown_timeout = container
            .get::<crate::RedisWorkerConfig>()
            .map(|cfg| cfg.shutdown_timeout)
            .unwrap_or_else(|| crate::RedisWorkerConfig::default().shutdown_timeout);

        monitor
            .shutdown_timeout(shutdown_timeout)
            .run_with_signal(async move {
                cancel.cancelled().await;
                Ok(())
            })
            .await?;
        Ok(())
    }
}

/// Build one apalis worker for a `ProcessMethod`. The wire payload is always
/// `serde_json::Value`; the port's `attempt` opens the envelope and the
/// macro-emitted `JobHandler` deserializes it to the user's `J`, so this
/// builder never names `J`.
fn build_worker(
    monitor: Monitor,
    conn: &RedisConnection,
    container: Container,
    method: &'static ProcessMethod,
) -> Monitor {
    // Fetch one job per poll. apalis 0.7 drives a fetched batch through a
    // `FuturesUnordered` and keeps polling while those futures are in flight, so
    // the buffer alone bounds nothing — it is `concurrency(1)` below that
    // serializes the work. Sizing the buffer to match matters anyway: a job
    // sitting in a saturated worker's buffer is invisible to every other
    // replica, which is exactly the throughput the deployment is paying for.
    // Namespaced under the queue name, which is how apalis routes a producer's
    // job to this worker.
    let storage: RedisStorage<serde_json::Value> = RedisStorage::new_with_config(
        conn.manager(),
        Config::default()
            .set_namespace(method.queue)
            .set_buffer_size(1),
    );
    // `consume::attempt` catches a handler panic itself (so the event lands
    // inside the per-job span rather than on the default panic hook), which
    // means this layer no longer sees one. It stays as the **backstop** for a
    // panic outside that call — in apalis's own fetch/deserialize path, or in
    // the closure prologue — where `RetryLayer` would not help either: it reacts
    // to `Err`, not to unwinding, so without a panic layer one bad job would
    // still take down the queue's consumer. Position is load-bearing: inside the
    // retry/error-handling layers, so an abort is not re-attempted.
    let worker = WorkerBuilder::new(method.queue)
        // The serialization point, and the outermost layer so the single permit
        // covers a job's whole lifecycle — retries included. apalis delegates
        // `poll_ready` to the inner service, so a held permit backs the fetch
        // loop off rather than piling work into memory: the next job stays in
        // Redis, where another replica can take it.
        .concurrency(1)
        .layer(ErrorHandlingLayer::new())
        .layer(RetryLayer::new(RetryPolicy::retries(method.retries)))
        .layer(CatchPanicLayer::new())
        .data(container)
        .backend(storage)
        .build_fn(
            move |job: serde_json::Value,
                  container: Data<Container>,
                  task_id: TaskId,
                  attempt: ApalisAttempt| async move {
                let container = (*container).clone();
                // The attempt is the port's; what this backend decides is
                // `translate` — how each outcome maps onto apalis's retry model.
                translate(
                    consume::attempt(
                        method,
                        job,
                        &task_id.to_string(),
                        attempt.current(),
                        container,
                    )
                    .await,
                )
            },
        );
    monitor.register(worker)
}

/// The error type apalis's `build_fn` closure returns.
type BoxDynError = Box<dyn std::error::Error + Send + Sync>;

/// The one thing this backend decides about an outcome: how it maps onto
/// apalis's retry model. A retryable failure stays a plain boxed error →
/// `Error::Failed`, which the retry budget re-attempts; a dead letter is
/// `Abort`, which `RetryLayer` skips so the job dead-letters at once.
fn translate(attempt: Attempt) -> Result<(), BoxDynError> {
    match attempt {
        Attempt::Ok => Ok(()),
        Attempt::Retry(error) => Err(error.source),
        Attempt::DeadLetter(error) => Err(abort(error.source)),
    }
}

/// Wrap an error as apalis's `Abort` — the shape that skips the retry budget
/// and dead-letters immediately.
fn abort(source: BoxDynError) -> BoxDynError {
    Box::new(apalis::prelude::Error::Abort(std::sync::Arc::new(source)))
}

#[cfg(test)]
mod tests {
    use nest_rs_queue::JobError;

    use super::*;

    /// The translation is three lines and nothing else exercises it: a swap of
    /// the two error arms would pass every suite, and a dead letter would burn
    /// the whole retry budget on a payload that cannot succeed.
    #[test]
    fn a_dead_letter_aborts_and_a_retry_does_not() {
        let dead = translate(Attempt::DeadLetter(JobError::abort("bad payload")))
            .expect_err("a dead letter fails the attempt");
        assert!(
            dead.downcast_ref::<apalis::prelude::Error>()
                .is_some_and(|e| matches!(e, apalis::prelude::Error::Abort(_))),
            "a dead letter is apalis's Abort, which skips the retry budget",
        );
        let retry = translate(Attempt::Retry(JobError::retry("upstream timed out")))
            .expect_err("a retryable failure fails this attempt");
        assert!(
            retry.downcast_ref::<apalis::prelude::Error>().is_none(),
            "a retryable failure is a plain error — Failed, re-attempted",
        );
        assert!(translate(Attempt::Ok).is_ok());
    }
}
