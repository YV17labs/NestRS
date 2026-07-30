//! apalis-redis `JobConsumer` exposed as a `Transport`: one apalis worker per
//! discovered `#[process]` method on a shared [`Monitor`].
//!
//! Every queue is consumed as `RedisStorage<serde_json::Value>` — the
//! backend-agnostic wire format — and dispatched through the type-erased
//! `JobHandler` the `#[processor]` macro emits.
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
use apalis::prelude::{Attempt, Data, Monitor, TaskId, WorkerBuilder, WorkerFactoryFn};
use async_trait::async_trait;
use nest_rs_core::{Container, ReachableProviders, Transport, inventory, panic_message};
use nest_rs_queue::ProcessMethod;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::connection::QueueConnection;

/// The consumer-side transport: drains the `#[processor]` inventory and runs
/// each job's process method against the Redis queue. Attached by
/// [`QueueWorkerModule`](crate::QueueWorkerModule).
pub struct QueueWorker {
    methods: Vec<&'static ProcessMethod>,
    container: Option<Container>,
}

impl QueueWorker {
    /// An empty worker; process methods and the container are wired at boot.
    pub fn new() -> Self {
        Self {
            methods: Vec::new(),
            container: None,
        }
    }
}

impl Default for QueueWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for QueueWorker {
    async fn configure(&mut self, container: &Container) -> Result<()> {
        // Drain link-time `#[process]` methods, filtered by ReachableProviders
        // so a method on a provider not in the app's module tree compiles in
        // but does not subscribe to its queue.
        let reachable = container.get::<ReachableProviders>();
        let mut methods: Vec<&'static ProcessMethod> = Vec::new();
        for entry in inventory::iter::<ProcessMethod>() {
            let provider_id = (entry.provider_type_id)();
            if let Some(r) = reachable.as_ref()
                && !r.0.contains(&provider_id)
            {
                tracing::warn!(
                    target: "nest_rs::queue",
                    processor = entry.name,
                    queue = entry.queue,
                    "skipped #[process] method: provider unreachable from app's module tree",
                );
                continue;
            }
            methods.push(entry);
        }
        self.methods = methods;

        // Fail fast at boot if methods exist but no connection is seeded.
        if !self.methods.is_empty() {
            container.get::<QueueConnection>().context(
                "QueueWorker found #[processor]s but no QueueConnection in the container — \
                 seed one with App::builder().provide_factory(|_| QueueConnection::connect(url))",
            )?;
            for m in &self.methods {
                tracing::info!(
                    target: "nest_rs::queue",
                    processor = m.name,
                    queue = m.queue,
                    retries = m.retries,
                    "registered queue processor",
                );
            }
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
            .expect("QueueWorker::configure must run before serve");
        let connection = container
            .get::<QueueConnection>()
            .expect("QueueConnection presence is verified in configure");

        let mut monitor = Monitor::new();
        for method in &self.methods {
            monitor = build_worker(monitor, &connection, container.clone(), method);
        }

        // Bound the post-signal drain so a hung `#[process]` can't block SIGTERM
        // until the orchestrator SIGKILLs the pod (QUEUE-I5). The config is a
        // factory output present in the container.
        let shutdown_timeout = container
            .get::<crate::QueueConfig>()
            .map(|cfg| cfg.shutdown_timeout)
            .unwrap_or_else(|| crate::QueueConfig::default().shutdown_timeout);

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
/// `serde_json::Value`; the macro-emitted `JobHandler` deserializes it to the
/// user's `J` inside the closure, so this builder never names `J`.
fn build_worker(
    monitor: Monitor,
    conn: &QueueConnection,
    container: Container,
    method: &ProcessMethod,
) -> Monitor {
    // Fetch one job per poll. apalis 0.7 drives a fetched batch through a
    // `FuturesUnordered` and keeps polling while those futures are in flight, so
    // the buffer alone bounds nothing — it is `concurrency(1)` below that
    // serializes the work. Sizing the buffer to match matters anyway: a job
    // sitting in a saturated worker's buffer is invisible to every other
    // replica, which is exactly the throughput the deployment is paying for.
    let storage = conn.consumer_storage(method.queue);
    let handler = method.handler;
    let queue_name = method.queue;
    let processor_name = method.name;
    // `run_job` catches a handler panic itself (so the event lands inside the
    // per-job span rather than on the default panic hook), which means this
    // layer no longer sees one. It stays as the **backstop** for a panic
    // outside that call — in apalis's own fetch/deserialize path, or in the
    // closure prologue — where `RetryLayer` would not help either: it reacts to
    // `Err`, not to unwinding, so without a panic layer one bad job would still
    // take down the queue's consumer. Position is load-bearing: inside the
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
                  attempt: Attempt| {
                let container = (*container).clone();
                // One span per job attempt; `attempt` distinguishes retries of
                // the same task_id. `.instrument` (not an entered guard held
                // across `.await`) keeps the span current for the whole poll.
                let span = tracing::info_span!(
                    target: "nest_rs::queue",
                    "process job",
                    queue = queue_name,
                    processor = processor_name,
                    job_id = %task_id,
                    attempt = attempt.current(),
                );
                async move {
                    tracing::debug!(
                        target: "nest_rs::queue",
                        attempt = attempt.current(),
                        "job started",
                    );
                    run_job(handler, job, container).await
                }
                .instrument(span)
            },
        );
    monitor.register(worker)
}

/// The error type apalis's `build_fn` closure returns.
type BoxDynError = Box<dyn std::error::Error + Send + Sync>;

/// Run one job handler and turn its outcome into the event it logs plus the
/// error apalis sees. Lifted out of the closure so every terminal state is
/// reachable from a test — the panic branch in particular, which is the one
/// that used to reach no event at all.
///
/// Three terminal states, one event each:
///
/// | outcome | event | apalis |
/// | --- | --- | --- |
/// | `Ok(())` | `job ok` (`info`) | success |
/// | non-retryable `Err` | `job dead-lettered: non-retryable failure` (`error`) | `Abort` |
/// | retryable `Err` | `job failed; will retry within the budget` (`warn`) | `Failed` |
/// | **panic** | `job dead-lettered: handler panicked` (`error`) | `Abort` |
///
/// The panic is caught **here** rather than left to the outer
/// `CatchPanicLayer`. That layer contains it correctly — the job fails, the
/// worker survives, the next job on the queue runs — but it unwinds past this
/// function, so the per-job span (`queue`, `processor`, `job_id`, `attempt`)
/// and every event below were skipped. The only trace of a panicking job was
/// the default Rust panic hook on stderr: no target, no fields, no span. At the
/// docs' own production filter (`nest_rs::queue=warn`) it vanished entirely,
/// while a deserialization failure on the same worker reported properly. The
/// outcome is unchanged; only the silence is gone.
async fn run_job(
    handler: nest_rs_queue::JobHandler,
    job: serde_json::Value,
    container: Container,
) -> Result<(), BoxDynError> {
    let started = std::time::Instant::now();
    let outcome = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(handler(
        job, container,
    )))
    .await;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    let result = match outcome {
        Ok(result) => result,
        Err(payload) => {
            let detail = panic_message(payload.as_ref());
            tracing::error!(
                target: "nest_rs::queue",
                elapsed_ms,
                panic = detail,
                "job dead-lettered: handler panicked",
            );
            // A panic is deterministic as far as the queue can tell — the same
            // payload panics again — so it aborts rather than burning the retry
            // budget. That is already what `CatchPanicLayer` did.
            return Err(abort(BoxDynError::from(detail.to_owned())));
        }
    };

    // Map the classified `JobError` onto apalis's retry model (QUEUE-I4). A
    // NON-retryable failure (deterministic: bad wire version, undeserializable
    // payload, missing provider, pipe rejection) aborts so `RetryLayer` skips
    // it and the job dead-letters at once. A retryable failure (the user
    // method's `Err`) stays a plain boxed error → `Error::Failed`, which the
    // retry budget re-attempts.
    match result {
        Ok(()) => {
            tracing::info!(target: "nest_rs::queue", elapsed_ms, "job ok");
            Ok(())
        }
        Err(je) if !je.retryable => {
            // `errors` carries the rejection's per-field detail when it had any —
            // same member name as the HTTP body and the WebSocket error frame, so
            // one query shape finds a validation failure on any transport. Absent
            // detail emits no field rather than an empty one.
            tracing::error!(
                target: "nest_rs::queue",
                elapsed_ms,
                error = %je,
                errors = je.details.as_ref().map(tracing::field::display),
                "job dead-lettered: non-retryable failure",
            );
            Err(abort(je.source))
        }
        Err(je) => {
            tracing::warn!(
                target: "nest_rs::queue",
                elapsed_ms,
                error = %je,
                "job failed; will retry within the budget",
            );
            Err(je.source)
        }
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
    use nest_rs_testing::LogCapture;

    use super::*;

    fn container() -> Container {
        Container::builder().build()
    }

    /// The finding: `CatchPanicLayer` dead-letters a panicking job correctly,
    /// and `nest_rs::queue` said **nothing** about it. The comparison case
    /// proves it was a gap rather than a choice — a deserialization failure on
    /// the same worker reports through `job dead-lettered: non-retryable
    /// failure`. The panic branch now emits the same shape.
    #[tokio::test]
    async fn a_panicking_handler_is_dead_lettered_with_an_event() {
        fn boom(
            _job: serde_json::Value,
            _c: Container,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>> {
            Box::pin(async { panic!("deliberate panic for panic-2") })
        }

        let logs = LogCapture::install();
        // The default hook would print the panic to stderr and drown the test
        // output; the event under test is the structured one.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = run_job(boom, serde_json::json!({}), container()).await;
        std::panic::set_hook(previous);

        let err = result.expect_err("a panicking job fails");
        assert!(
            err.downcast_ref::<apalis::prelude::Error>()
                .is_some_and(|e| matches!(e, apalis::prelude::Error::Abort(_))),
            "a panic is deterministic — it dead-letters instead of burning the retry budget",
        );

        let event = logs.expect_one("nest_rs::queue", "job dead-lettered: handler panicked");
        assert_eq!(
            event.level, "error",
            "at the docs' own production filter (`nest_rs::queue=warn`) it has to show",
        );
        assert_eq!(
            event.field("panic").as_deref(),
            Some("deliberate panic for panic-2"),
            "the panic message rides on the shared `panic` field: {event:#?}",
        );
        assert!(event.field("elapsed_ms").is_some());
    }

    /// A dead-lettered job is read from a log, days later, by someone who cannot
    /// re-run it. `error=validation failed` alone does not say which field of
    /// which payload was wrong — and the rejection knew. The detail rides the
    /// event as `errors`, the member name HTTP and the WebSocket error frame use
    /// for the same failure.
    #[tokio::test]
    async fn a_dead_lettered_pipe_rejection_logs_its_field_errors() {
        fn rejected(
            _job: serde_json::Value,
            _c: Container,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>> {
            Box::pin(async {
                Err(
                    JobError::abort("validation failed").with_details(Some(serde_json::json!({
                        "slug": [{ "code": "length" }],
                    }))),
                )
            })
        }

        let logs = LogCapture::install();
        assert!(
            run_job(rejected, serde_json::json!({}), container())
                .await
                .is_err()
        );

        let event = logs.expect_one("nest_rs::queue", "job dead-lettered: non-retryable failure");
        let errors = event
            .field("errors")
            .unwrap_or_else(|| panic!("the dead-letter event carries `errors`: {event:#?}"));
        assert!(
            errors.contains("slug"),
            "and it names the offending field: {errors}",
        );
    }

    /// A failure with nothing structured to say must not invent an `errors`
    /// field — an empty one reads as "checked, nothing found".
    #[tokio::test]
    async fn a_dead_letter_without_detail_logs_no_errors_field() {
        fn bare(
            _job: serde_json::Value,
            _c: Container,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>> {
            Box::pin(async { Err(JobError::abort("missing field `id`")) })
        }

        let logs = LogCapture::install();
        assert!(
            run_job(bare, serde_json::json!({}), container())
                .await
                .is_err()
        );
        let event = logs.expect_one("nest_rs::queue", "job dead-lettered: non-retryable failure");
        assert!(
            event.field("errors").is_none(),
            "no detail ⇒ no field: {event:#?}",
        );
    }

    /// The three non-panic outcomes, so the panic branch is pinned against
    /// siblings that already worked rather than in isolation.
    #[tokio::test]
    async fn every_other_outcome_keeps_its_own_event() {
        fn ok(
            _job: serde_json::Value,
            _c: Container,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>> {
            Box::pin(async { Ok(()) })
        }
        fn fatal(
            _job: serde_json::Value,
            _c: Container,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>> {
            Box::pin(async { Err(JobError::abort("missing field `id`")) })
        }
        fn transient(
            _job: serde_json::Value,
            _c: Container,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>> {
            Box::pin(async { Err(JobError::retry("upstream timed out")) })
        }

        let logs = LogCapture::install();
        assert!(
            run_job(ok, serde_json::json!({}), container())
                .await
                .is_ok()
        );
        assert!(
            run_job(fatal, serde_json::json!({}), container())
                .await
                .is_err()
        );
        assert!(
            run_job(transient, serde_json::json!({}), container())
                .await
                .is_err()
        );

        assert_eq!(logs.expect_one("nest_rs::queue", "job ok").level, "info");
        assert_eq!(
            logs.expect_one("nest_rs::queue", "job dead-lettered: non-retryable failure")
                .level,
            "error",
        );
        assert_eq!(
            logs.expect_one("nest_rs::queue", "job failed; will retry within the budget")
                .level,
            "warn",
        );
    }
}
