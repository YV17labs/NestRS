//! The port's half of consuming — what a job attempt *is*, written once for
//! every adapter.
//!
//! [`discover`] drains the `#[process]` inventory the way every backend must:
//! module-gated, with the inert-host `warn`, refusing two processors on one
//! queue. [`attempt`] runs one attempt the way every backend must: it opens the
//! envelope, continues or mints the trace, opens the `queue.job` span and the
//! ambient scope, catches a panic, classifies the outcome and files the three
//! events and the `nest_rs::operation` line. What it returns is an [`Attempt`],
//! and an adapter's consumer is a fetch loop that calls it and translates that
//! into its backend's vocabulary — apalis `Abort`/`Failed`, a NATS consumer's
//! `ack`/`nak`/`term`. Nothing in here names a backend; nothing in an adapter
//! restates what is here.

use std::sync::Arc;

use nest_rs_core::{
    Container, ReachableProviders, RequestScope, panic_message, with_request_scope,
};
use tracing::Instrument;

use crate::inventory::JobError;
use crate::inventory::{ProcessMethod, check_duplicate_queue_claims};
use crate::{TARGET, envelope, unit};

/// The `#[process]` methods this app serves: every entry whose provider is
/// reachable from the running app's root, with a boot `warn` for each that is
/// linked but unreachable, and a boot error when two claim one queue.
///
/// Called once by an adapter's `Transport::configure`; what it returns is what
/// that adapter subscribes to.
pub fn discover(container: &Container) -> anyhow::Result<Vec<&'static ProcessMethod>> {
    // Filtered by ReachableProviders so a method on a provider not in the app's
    // module tree compiles in but does not subscribe to its queue.
    let reachable = container.get::<ReachableProviders>();
    let mut methods: Vec<&'static ProcessMethod> = Vec::new();
    for entry in nest_rs_core::inventory::iter::<ProcessMethod>() {
        if !ReachableProviders::reaches(reachable.as_deref(), (entry.provider_type_id)()) {
            ::nest_rs_core::report_inert_host!(
                target: TARGET,
                what: "#[process] method",
                origin: entry.origin,
                processor = entry.name,
                queue = entry.queue,
            );
            continue;
        }
        methods.push(entry);
    }
    // Aggregating a queue is like aggregating a mount: the one failure mode it
    // adds is two contributions claiming one addressable name, and that is a
    // boot error naming both. Checked after module-gating, so a processor
    // another app owns cannot fail this app's boot.
    check_duplicate_queue_claims(&methods).map_err(anyhow::Error::msg)?;
    for m in &methods {
        tracing::info!(
            target: TARGET,
            processor = m.name,
            queue = m.queue,
            retries = m.retries,
            "registered queue processor",
        );
    }
    Ok(methods)
}

/// How one attempt ended, in the port's vocabulary. The adapter translates it
/// into its backend's — and nothing else about the outcome is its to decide.
#[derive(Debug)]
pub enum Attempt {
    /// The handler returned `Ok`.
    Ok,
    /// The handler failed in a way a re-run could fix (the user method's
    /// `Err`): the backend re-attempts within the method's retry budget.
    Retry(JobError),
    /// The handler failed deterministically — an undeserializable payload, a
    /// pipe rejection, a missing provider, a panic — so re-running would burn
    /// the budget on a payload that cannot succeed: the backend dead-letters
    /// it at once.
    DeadLetter(JobError),
}

/// Run one attempt of `method` over the wire `payload` — the whole of what an
/// attempt is, from the envelope to the line that reports it.
///
/// `job_id` and `attempt` are the backend's identifiers for the task and its
/// attempt number; they ride the span and the operation line so retries of one
/// task are one `job_id` and distinct `attempt`s. The id is taken **by value**
/// because the line owns it for the length of the attempt: borrowing it made
/// every adapter allocate the string and this function allocate it again.
pub async fn attempt(
    method: &'static ProcessMethod,
    payload: serde_json::Value,
    job_id: String,
    attempt: usize,
    container: Container,
) -> Attempt {
    // The producer sealed its W3C trace context into the payload, because a
    // queue is the one hop the framework crosses that is a *process* boundary
    // rather than a task one. Continuing it here is what makes one trace span
    // the whole chain: the HTTP request that enqueued, and this worker minutes
    // later in another binary, are one trace and the job is a child of the
    // enqueue. A bare payload — the raw hatch, an older producer, a foreign
    // system — starts a trace instead; see `envelope`.
    let (job, inherited) = envelope::open(payload);
    let continued_trace = inherited.is_some();
    let correlation = inherited.unwrap_or_else(nest_rs_core::Correlation::mint);
    // One span per job attempt; `attempt` distinguishes retries of the same
    // job_id. `.instrument` (not an entered guard held across `.await`) keeps
    // the span current for the whole poll. Through `operation_span!` so a job
    // declares the same canonical fields every edge does — `actor_id`
    // included, which is what lets a job's events be attributed at all.
    let span = nest_rs_core::operation_span!(
        target: TARGET,
        // A job is delivered *to* this process — the kind a messaging view
        // classifies on.
        kind: nest_rs_core::operation_log::kind::CONSUMER,
        unit::JOB,
        &correlation,
        queue = method.queue,
        processor = method.name,
        job_id = %job_id,
        attempt,
        // Whether this job is traceable back to what enqueued it, or starts a
        // trace of its own. An operator chasing a lost request needs to tell
        // the two apart.
        continued_trace,
    );
    // What the job's own line reports it *was*. The span carries the same facts
    // for the export; a log line renders no span state, so the line that names
    // the work has to carry them as event attributes of its own.
    let identity = JobIdentity {
        queue: method.queue,
        processor: method.name,
        job_id,
        attempt,
    };
    async move {
        tracing::debug!(target: TARGET, attempt = identity.attempt, "job started");
        // The ambient context too, not just the span: a `#[process]` body that
        // enqueues a follow-up job must seal *this* id, not mint a third one and
        // break the chain.
        let scope = Arc::new(RequestScope::new(container.clone()));
        with_request_scope(
            Some(scope),
            correlation,
            run(method.handler, job, container, identity),
        )
        .await
    }
    .instrument(span)
    .await
}

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
/// [`Attempt`] the adapter translates. Lifted out of [`attempt`] so every
/// terminal state is reachable from a test — the panic branch in particular,
/// which is the one that used to reach no event at all.
///
/// Three terminal states, one event each:
///
/// | outcome | event | [`Attempt`] |
/// | --- | --- | --- |
/// | `Ok(())` | *(the operation line alone)* | `Ok` |
/// | non-retryable `Err` | `job dead-lettered: non-retryable failure` (`error`) | `DeadLetter` |
/// | retryable `Err` | `job failed; will retry within the budget` (`warn`) | `Retry` |
/// | **panic** | `job dead-lettered: handler panicked` (`error`) | `DeadLetter` |
///
/// The panic is caught **here** rather than left to a backend's panic layer.
/// Such a layer contains it correctly — the job fails, the worker survives, the
/// next job on the queue runs — but it unwinds past this function, so the
/// per-job span (`queue`, `processor`, `job_id`, `attempt`) and every event
/// below were skipped. The only trace of a panicking job was the default Rust
/// panic hook on stderr: no target, no fields, no span. At the docs' own
/// production filter (`nest_rs::queue=warn`) it vanished entirely, while a
/// deserialization failure on the same worker reported properly. The outcome is
/// unchanged; only the silence is gone.
async fn run(
    handler: crate::JobHandler,
    job: serde_json::Value,
    container: Container,
    identity: JobIdentity,
) -> Attempt {
    let started = std::time::Instant::now();
    let outcome = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(handler(
        job, container,
    )))
    .await;
    // Every terminal state, one detail event and one line. The detail says
    // *why* and stays on `nest_rs::queue`; the line says the job ran, and is the
    // family's — so `nest_rs::operation` answers "what did this worker do" the
    // same way it answers it for a request. Neither restates the other's fields.
    let (settled, result) = match outcome {
        Ok(Ok(())) => (nest_rs_core::operation_log::OK, Attempt::Ok),
        // A NON-retryable failure (deterministic: bad wire version,
        // undeserializable payload, missing provider, pipe rejection)
        // dead-letters at once. A retryable failure (the user method's `Err`)
        // is re-attempted within the budget.
        Ok(Err(je)) if !je.retryable => {
            // `errors` carries the rejection's per-field detail when it had any —
            // same member name as the HTTP body and the WebSocket error frame, so
            // one query shape finds a validation failure on any transport. Absent
            // detail emits no field rather than an empty one.
            tracing::error!(
                target: TARGET,
                error = %je,
                errors = je.details.as_ref().map(tracing::field::display),
                "job dead-lettered: non-retryable failure",
            );
            (nest_rs_core::operation_log::ERROR, Attempt::DeadLetter(je))
        }
        Ok(Err(je)) => {
            tracing::warn!(
                target: TARGET,
                error = %je,
                "job failed; will retry within the budget",
            );
            (nest_rs_core::operation_log::ERROR, Attempt::Retry(je))
        }
        Err(payload) => {
            let detail = panic_message(payload.as_ref()).to_owned();
            tracing::error!(
                target: TARGET,
                panic = %detail,
                "job dead-lettered: handler panicked",
            );
            // A panic is deterministic as far as the queue can tell — the same
            // payload panics again — so it dead-letters rather than burning the
            // retry budget.
            (
                nest_rs_core::operation_log::PANIC,
                Attempt::DeadLetter(JobError::abort(detail)),
            )
        }
    };

    tracing::info!(
        name: unit::JOB,
        target: nest_rs_core::operation_log::TARGET,
        message = unit::JOB,
        queue = identity.queue,
        processor = identity.processor,
        job_id = identity.job_id,
        attempt = identity.attempt,
        outcome = settled,
        duration_ms = nest_rs_core::operation_log::duration_ms(started),
    );
    result
}

#[cfg(test)]
mod tests {
    use std::future::Future;

    use nest_rs_testing::LogCapture;

    use super::*;

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

    fn container() -> Container {
        Container::builder().build()
    }

    type Handler = std::pin::Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>>;

    /// The finding: a backend's panic layer dead-letters a panicking job
    /// correctly, and `nest_rs::queue` said **nothing** about it. The comparison
    /// case proves it was a gap rather than a choice — a deserialization failure
    /// on the same worker reports through `job dead-lettered: non-retryable
    /// failure`. The panic branch now emits the same shape.
    #[tokio::test]
    async fn a_panicking_handler_is_dead_lettered_with_an_event() {
        fn boom(_job: serde_json::Value, _c: Container) -> Handler {
            Box::pin(async { panic!("deliberate panic for panic-2") })
        }

        let logs = LogCapture::install();
        // The default hook would print the panic to stderr and drown the test
        // output; the event under test is the structured one.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let result = run(boom, serde_json::json!({}), container(), identity()).await;
        std::panic::set_hook(previous);

        assert!(
            matches!(result, Attempt::DeadLetter(_)),
            "a panic is deterministic — it dead-letters instead of burning the retry budget",
        );

        let event = logs.expect_one(TARGET, "job dead-lettered: handler panicked");
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
        let ran = logs.expect_one(nest_rs_core::operation_log::TARGET, unit::JOB);
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
        fn rejected(_job: serde_json::Value, _c: Container) -> Handler {
            Box::pin(async {
                Err(
                    JobError::abort("validation failed").with_details(Some(serde_json::json!({
                        "slug": [{ "code": "length" }],
                    }))),
                )
            })
        }

        let logs = LogCapture::install();
        let result = run(rejected, serde_json::json!({}), container(), identity()).await;
        // The classification, not merely the failure. A non-retryable error that
        // stopped dead-lettering would spend the whole retry budget re-running a
        // payload that cannot succeed, and every assertion below would still
        // pass — the one silent way this path can break.
        assert!(
            matches!(result, Attempt::DeadLetter(_)),
            "a non-retryable failure dead-letters so the budget is never spent on it",
        );

        let event = logs.expect_one(TARGET, "job dead-lettered: non-retryable failure");
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
        fn bare(_job: serde_json::Value, _c: Container) -> Handler {
            Box::pin(async { Err(JobError::abort("missing field `id`")) })
        }

        let logs = LogCapture::install();
        assert!(matches!(
            run(bare, serde_json::json!({}), container(), identity()).await,
            Attempt::DeadLetter(_)
        ));
        let event = logs.expect_one(TARGET, "job dead-lettered: non-retryable failure");
        assert!(
            event.field("errors").is_none(),
            "no detail ⇒ no field: {event:#?}",
        );
    }

    /// The three non-panic outcomes, so the panic branch is pinned against
    /// siblings that already worked rather than in isolation.
    #[tokio::test]
    async fn every_other_outcome_keeps_its_own_event() {
        fn ok(_job: serde_json::Value, _c: Container) -> Handler {
            Box::pin(async { Ok(()) })
        }
        fn fatal(_job: serde_json::Value, _c: Container) -> Handler {
            Box::pin(async { Err(JobError::abort("missing field `id`")) })
        }
        fn transient(_job: serde_json::Value, _c: Container) -> Handler {
            Box::pin(async { Err(JobError::retry("upstream timed out")) })
        }

        let logs = LogCapture::install();
        assert!(matches!(
            run(ok, serde_json::json!({}), container(), identity()).await,
            Attempt::Ok
        ));
        assert!(matches!(
            run(fatal, serde_json::json!({}), container(), identity()).await,
            Attempt::DeadLetter(_)
        ));
        assert!(matches!(
            run(transient, serde_json::json!({}), container(), identity()).await,
            Attempt::Retry(_)
        ));

        // Success is said once, and it is the family's line that says it.
        assert!(
            logs.find(TARGET, "job ok").is_empty(),
            "a successful job reports through `nest_rs::operation`, not twice",
        );
        assert_eq!(
            logs.expect_one(TARGET, "job dead-lettered: non-retryable failure")
                .level,
            "error",
        );
        assert_eq!(
            logs.expect_one(TARGET, "job failed; will retry within the budget")
                .level,
            "warn",
        );
    }

    /// The retryable half of the same classification, and the one with no
    /// visible outcome at all: the backend re-attempts the job, so a transient
    /// failure that eventually succeeds leaves the queue looking healthy.
    ///
    /// Which is exactly when it matters — a job succeeding on attempt four
    /// every time is a system about to fall over, and this `warn` is the only
    /// signal before it does.
    #[tokio::test]
    async fn a_retryable_failure_is_reported_before_the_budget_re_attempts_it() {
        fn flaky(_job: serde_json::Value, _c: Container) -> Handler {
            Box::pin(async { Err(JobError::retry("the upstream API timed out")) })
        }

        let logs = LogCapture::install();
        let result = run(flaky, serde_json::json!({}), container(), identity()).await;
        assert!(
            matches!(result, Attempt::Retry(_)),
            "a retryable failure is a `Retry` — that is what keeps the budget alive",
        );

        let event = logs.expect_one(TARGET, "job failed; will retry within the budget");
        assert_eq!(event.level, "warn");
        assert!(
            event
                .field("error")
                .is_some_and(|e| e.contains("upstream API")),
            "the event carries the cause the retry will hit again, got {:?}",
            event.fields,
        );
        let ran = logs.expect_one(nest_rs_core::operation_log::TARGET, unit::JOB);
        assert_eq!(
            ran.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::ERROR),
        );
        assert_eq!(ran.field("queue").as_deref(), Some("audio"));
        assert!(ran.field("duration_ms").is_some());
    }
}
