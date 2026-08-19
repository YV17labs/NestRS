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
                ::nest_rs_core::report_inert_host!(
                    target: nest_rs_queue::TARGET,
                    what: "#[process] method",
                    origin: entry.origin,
                    processor = entry.name,
                    queue = entry.queue,
                );
                continue;
            }
            methods.push(entry);
        }
        // Aggregating a queue is like aggregating a mount: the one failure mode
        // it adds is two contributions claiming one addressable name, and that
        // is a boot error naming both. Checked after module-gating, so a
        // processor another app owns cannot fail this app's boot.
        nest_rs_queue::check_duplicate_queue_claims(&methods).map_err(anyhow::Error::msg)?;

        self.methods = methods;

        // Fail fast at boot if methods exist but no connection is seeded.
        if !self.methods.is_empty() {
            container.get::<QueueConnection>().context(
                "QueueWorker found #[processor]s but no QueueConnection in the container — \
                 seed one with App::builder().provide_factory(|_| QueueConnection::connect(url))",
            )?;
            for m in &self.methods {
                tracing::info!(
                    target: nest_rs_queue::TARGET,
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
                // The producer sealed its W3C trace context into the payload,
                // because a queue is the one hop the framework crosses that is a
                // *process* boundary rather than a task one. Continuing it here
                // is what makes one trace span the whole chain: the HTTP request
                // that enqueued, and this worker minutes later in another binary,
                // are one trace and the job is a child of the enqueue. A bare
                // payload — the raw hatch, an older producer, a foreign system —
                // starts a trace instead; see `nest_rs_queue::envelope`.
                let (job, inherited) = nest_rs_queue::envelope::open(job);
                let continued_trace = inherited.is_some();
                let correlation = inherited.unwrap_or_else(nest_rs_core::Correlation::mint);
                // One span per job attempt; `attempt` distinguishes retries of
                // the same task_id. `.instrument` (not an entered guard held
                // across `.await`) keeps the span current for the whole poll.
                // Through `operation_span!` so a job declares the same canonical
                // fields every edge does — `actor_id` included, which is what
                // lets a job's events be attributed at all.
                let span = nest_rs_core::operation_span!(
                    target: nest_rs_queue::TARGET,
                    // A job is delivered *to* this process — the kind a
                    // messaging view classifies on.
                    kind: nest_rs_core::operation_log::kind::CONSUMER,
                    nest_rs_core::operation_log::unit::QUEUE_JOB,
                    &correlation,
                    queue = queue_name,
                    processor = processor_name,
                    job_id = %task_id,
                    attempt = attempt.current(),
                    // Whether this job is traceable back to what enqueued it, or
                    // starts a trace of its own. An operator chasing a lost
                    // request needs to tell the two apart.
                    continued_trace,
                );
                // What the job's own line reports it *was*. The span carries
                // the same facts for the export; a log line renders no span
                // state, so the line that names the work has to carry them as
                // event attributes of its own.
                let identity = JobIdentity {
                    queue: queue_name,
                    processor: processor_name,
                    job_id: task_id.to_string(),
                    attempt: attempt.current(),
                };
                async move {
                    tracing::debug!(
                        target: nest_rs_queue::TARGET,
                        attempt = identity.attempt,
                        "job started",
                    );
                    // The ambient context too, not just the span: a `#[process]`
                    // body that enqueues a follow-up job must seal *this* id, not
                    // mint a third one and break the chain.
                    let scope =
                        std::sync::Arc::new(nest_rs_core::RequestScope::new(container.clone()));
                    nest_rs_core::with_request_scope(
                        Some(scope),
                        correlation,
                        None,
                        run_job(handler, job, container, identity),
                    )
                    .await
                }
                .instrument(span)
            },
        );
    monitor.register(worker)
}

/// The error type apalis's `build_fn` closure returns.
type BoxDynError = Box<dyn std::error::Error + Send + Sync>;

/// What one job attempt is, for the line that reports it ran.
///
/// Held as a value rather than read back off the span: `tracing` gives no way to
/// read a span's fields, and a log line renders none of them anyway.
struct JobIdentity {
    queue: &'static str,
    processor: &'static str,
    job_id: String,
    attempt: usize,
}

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
    identity: JobIdentity,
) -> Result<(), BoxDynError> {
    let started = std::time::Instant::now();
    let outcome = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(handler(
        job, container,
    )))
    .await;
    // Every terminal state, one detail event and one line. The detail says
    // *why* and stays on `nest_rs::queue`; the line says the job ran, and is the
    // family's — so `nest_rs::operation` answers "what did this worker do" the same
    // way it answers it for a request. Neither restates the other's fields.
    let (settled, result) = match outcome {
        Ok(Ok(())) => (nest_rs_core::operation_log::OK, Ok(())),
        // Map the classified `JobError` onto apalis's retry model (QUEUE-I4). A
        // NON-retryable failure (deterministic: bad wire version, undeserializable
        // payload, missing provider, pipe rejection) aborts so `RetryLayer` skips
        // it and the job dead-letters at once. A retryable failure (the user
        // method's `Err`) stays a plain boxed error → `Error::Failed`, which the
        // retry budget re-attempts.
        Ok(Err(je)) if !je.retryable => {
            // `errors` carries the rejection's per-field detail when it had any —
            // same member name as the HTTP body and the WebSocket error frame, so
            // one query shape finds a validation failure on any transport. Absent
            // detail emits no field rather than an empty one.
            tracing::error!(
                target: nest_rs_queue::TARGET,
                error = %je,
                errors = je.details.as_ref().map(tracing::field::display),
                "job dead-lettered: non-retryable failure",
            );
            (nest_rs_core::operation_log::ERROR, Err(abort(je.source)))
        }
        Ok(Err(je)) => {
            tracing::warn!(
                target: nest_rs_queue::TARGET,
                error = %je,
                "job failed; will retry within the budget",
            );
            (nest_rs_core::operation_log::ERROR, Err(je.source))
        }
        Err(payload) => {
            let detail = panic_message(payload.as_ref()).to_owned();
            tracing::error!(
                target: nest_rs_queue::TARGET,
                panic = %detail,
                "job dead-lettered: handler panicked",
            );
            // A panic is deterministic as far as the queue can tell — the same
            // payload panics again — so it aborts rather than burning the retry
            // budget. That is already what `CatchPanicLayer` did.
            (
                nest_rs_core::operation_log::PANIC,
                Err(abort(BoxDynError::from(detail))),
            )
        }
    };

    tracing::info!(
        name: nest_rs_core::operation_log::unit::QUEUE_JOB,
        target: nest_rs_core::operation_log::TARGET,
        message = nest_rs_core::operation_log::unit::QUEUE_JOB,
        queue = identity.queue,
        processor = identity.processor,
        job_id = identity.job_id,
        attempt = identity.attempt,
        outcome = settled,
        duration_ms = nest_rs_core::operation_log::duration_ms(started),
    );
    result
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

    /// The identity a test job reports it was. The assertions are about the
    /// outcome and the line that carries it, so one fixture serves them all.
    fn identity() -> JobIdentity {
        JobIdentity {
            queue: "audio",
            processor: "AudioProcessor",
            job_id: "01a0".to_owned(),
            attempt: 1,
        }
    }

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
        let result = run_job(boom, serde_json::json!({}), container(), identity()).await;
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
        // The duration is on the family's line, not restated on the detail.
        let ran = logs.expect_one(
            nest_rs_core::operation_log::TARGET,
            nest_rs_core::operation_log::unit::QUEUE_JOB,
        );
        assert_eq!(
            ran.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::PANIC),
            "a panic is its own outcome, not a plain error: {ran:#?}",
        );
        assert!(ran.field("duration_ms").is_some());
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
        let err = run_job(rejected, serde_json::json!({}), container(), identity())
            .await
            .expect_err("a rejected payload fails");
        // The classification, not merely the failure. A non-retryable error that
        // stopped aborting would spend the whole retry budget re-running a
        // payload that cannot succeed, and every assertion below would still
        // pass — the one silent way this path can break.
        assert!(
            err.downcast_ref::<apalis::prelude::Error>()
                .is_some_and(|e| matches!(e, apalis::prelude::Error::Abort(_))),
            "a non-retryable failure aborts so the budget is never spent on it",
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
            run_job(bare, serde_json::json!({}), container(), identity())
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
            run_job(ok, serde_json::json!({}), container(), identity())
                .await
                .is_ok()
        );
        assert!(
            run_job(fatal, serde_json::json!({}), container(), identity())
                .await
                .is_err()
        );
        assert!(
            run_job(transient, serde_json::json!({}), container(), identity())
                .await
                .is_err()
        );

        // Success is said once, and it is the family's line that says it.
        assert!(
            logs.find("nest_rs::queue", "job ok").is_empty(),
            "a successful job reports through `nest_rs::operation`, not twice",
        );
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

    /// The retryable half of the same classification, and the one with no
    /// visible outcome at all: apalis re-attempts the job, so a transient
    /// failure that eventually succeeds leaves the queue looking healthy.
    ///
    /// Which is exactly when it matters — a job succeeding on attempt four
    /// every time is a system about to fall over, and this `warn` is the only
    /// signal before it does. It stays a plain boxed error rather than an
    /// `Abort`, which is what tells apalis to retry rather than dead-letter.
    #[tokio::test]
    async fn a_retryable_failure_is_reported_before_the_budget_re_attempts_it() {
        fn flaky(
            _job: serde_json::Value,
            _c: Container,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>> {
            Box::pin(async { Err(JobError::retry("the upstream API timed out")) })
        }

        let logs = LogCapture::install();
        let err = run_job(flaky, serde_json::json!({}), container(), identity())
            .await
            .expect_err("a retryable failure still fails this attempt");
        assert!(
            err.downcast_ref::<apalis::prelude::Error>().is_none(),
            "a retryable failure is *not* an Abort — that is what keeps the budget alive",
        );

        let event = logs.expect_one("nest_rs::queue", "job failed; will retry within the budget");
        assert_eq!(event.level, "warn");
        assert!(
            event
                .field("error")
                .is_some_and(|e| e.contains("upstream API")),
            "the event carries the cause the retry will hit again, got {:?}",
            event.fields,
        );
        let ran = logs.expect_one(
            nest_rs_core::operation_log::TARGET,
            nest_rs_core::operation_log::unit::QUEUE_JOB,
        );
        assert_eq!(
            ran.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::ERROR),
        );
        assert_eq!(ran.field("queue").as_deref(), Some("audio"));
        assert!(ran.field("duration_ms").is_some());
    }
}
