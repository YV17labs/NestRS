//! The per-method inventory entry — the link-time seam between the
//! `#[processor]` macro and any backend.
//!
//! Type-erased on purpose: the [`JobHandler`] receives a `serde_json::Value`
//! and deserializes to the method's job type inside the closure the macro
//! emits. This frees the backend from naming the user's `J` and frees the
//! inventory from carrying backend-specific function pointers.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;

use nest_rs_core::Container;

/// Wire-format version every backend wraps jobs with on push and unwraps on
/// dispatch. Bumping it lets a `#[processor]` handler reject payloads from a
/// newer release (rolling-deploy safety) instead of misinterpreting bytes.
///
/// The envelope is `{ "v": <number>, "payload": <user payload> }`. An
/// **unversioned** value — anything that isn't an object with both `v` and
/// `payload` keys — is treated as a legacy raw payload and decoded directly
/// as the job type (with a warning), so jobs left in Redis from a prior
/// deploy still drain.
pub const WIRE_FORMAT_VERSION: u32 = 1;

/// A job failure classified for the backend's retry policy (QUEUE-I4).
///
/// A **retryable** failure ([`retry`](JobError::retry)) is a transient fault —
/// the user `#[process]` method returning `Err` — that a re-attempt might clear.
/// A **non-retryable** failure ([`abort`](JobError::abort)) is *deterministic*
/// (an unsupported wire-format version, an undeserializable payload, a missing
/// provider): retrying it burns the retry budget re-failing identically before
/// the job dead-letters. A backend must abort a non-retryable failure at once
/// and surface it (an `error!` at dead-letter) instead of silently retrying.
pub struct JobError {
    /// Whether the backend's retry layer should re-attempt this job.
    pub retryable: bool,
    /// The underlying error, for logging and the backend's dead-letter record.
    pub source: Box<dyn std::error::Error + Send + Sync>,
    /// Structured detail the failure carried, when it had any — the per-field
    /// errors of a `Valid<T>` job-argument rejection.
    ///
    /// A dead-lettered job is read from a log, days later, by someone who cannot
    /// re-run it: `error=validation failed` alone does not say which field of
    /// which payload was wrong, and the information existed at the moment of
    /// failure. A backend surfaces this beside the error on the dead-letter
    /// event, under the same `errors` name HTTP and WebSockets use.
    pub details: Option<serde_json::Value>,
}

impl JobError {
    /// A **retryable** failure (a transient fault worth re-attempting).
    pub fn retry(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self {
            retryable: true,
            source: source.into(),
            details: None,
        }
    }

    /// A **non-retryable** failure (deterministic — retrying it re-fails
    /// identically): abort and dead-letter immediately.
    pub fn abort(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self {
            retryable: false,
            source: source.into(),
            details: None,
        }
    }

    /// The failure of a job that ran fine and whose data context could not
    /// honour it — a transaction that could not be committed, or one whose
    /// handle outlived the attempt.
    ///
    /// **The context's classification is this crate's**: a retry replays the
    /// job body, so a commit failure that will repeat identically — a deferred
    /// constraint violation, a commit whose outcome is unknown — costs the
    /// whole budget in replayed side effects and dead-letters anyway. A
    /// serialization conflict is the case worth another attempt, and the only
    /// thing that says which is the database.
    pub fn unhonoured(unhonoured: nest_rs_worker::Unhonoured) -> Self {
        Self {
            retryable: unhonoured.retryable,
            source: unhonoured.reason.into(),
            details: None,
        }
    }

    /// Attach structured detail to a failure — what a rejected pipe knows about
    /// *which* field failed.
    pub fn with_details(mut self, details: Option<serde_json::Value>) -> Self {
        self.details = details;
        self
    }
}

impl std::fmt::Display for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.source, f)
    }
}

impl std::fmt::Debug for JobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobError")
            .field("retryable", &self.retryable)
            .field("source", &self.source)
            .field("details", &self.details)
            .finish()
    }
}

impl std::error::Error for JobError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.source)
    }
}

/// Type-erased async job handler the `#[processor]` macro emits for each
/// `#[process]` method. Backends invoke it with a JSON payload pulled off
/// their wire; the closure deserializes to the user's job type, resolves the
/// provider from the container, and dispatches. A returned [`JobError`] tells
/// the backend whether the failure is worth retrying.
pub type JobHandler = fn(
    payload: serde_json::Value,
    container: Container,
) -> Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>>;

/// Link-time inventory entry submitted by `#[processor]` for each
/// `#[process]`-tagged method. A backend's `Transport` drains this registry at
/// boot and filters by
/// [`ReachableProviders`](::nest_rs_core::ReachableProviders) so a method on a
/// provider not reachable from the app's module tree is skipped with a boot
/// `warn` (the consumer logs it, so leftover code stays visible).
pub struct ProcessMethod {
    /// The process method's name, for boot logs.
    pub name: &'static str,
    /// The queue name this method drains.
    pub queue: &'static str,
    /// Retry budget per job before it is considered failed.
    pub retries: usize,
    /// `TypeId` of the host provider, matched against the reachable set to
    /// module-gate this consumer.
    pub provider_type_id: fn() -> TypeId,
    /// The type-erased handler that resolves the provider and runs the method.
    pub handler: JobHandler,
}

::nest_rs_core::inventory::collect!(ProcessMethod);

/// Boot check: two `#[process]` methods may not drain one queue.
///
/// Backend-agnostic on purpose — any backend draining this registry owes the
/// same refusal, so the rule lives beside the registry rather than inside
/// whichever consumer happens to be wired.
///
/// A backend builds **one worker per entry**, so two claimants become two
/// consumers polling the same stream: each job goes to whichever pops it first,
/// which is neither the developer's choice nor a stable one across runs. The
/// retry budget forks with it — the same job gets one attempt or nine depending
/// on which worker won it.
///
/// A queue is addressed by name and carries exactly one job type
/// (`#[process(queue = Q)]` asserts the handler's payload is `Q::Job`), so
/// draining it twice is never the shape a developer meant: the way to run more
/// jobs at once is the backend's own concurrency, not a second handler.
pub fn check_duplicate_queue_claims(methods: &[&ProcessMethod]) -> Result<(), String> {
    let mut claimants: std::collections::BTreeMap<&'static str, Vec<&'static str>> =
        std::collections::BTreeMap::new();
    for method in methods {
        claimants.entry(method.queue).or_default().push(method.name);
    }

    let clashes: Vec<String> = claimants
        .into_iter()
        .filter(|(_, names)| names.len() > 1)
        .map(|(queue, names)| format!("{queue:?} ({})", names.join(" and ")))
        .collect();

    if clashes.is_empty() {
        return Ok(());
    }
    Err(format!(
        "duplicate queue claim: {} — a queue is drained by one `#[process]` \
         method, so a second one would take an unpredictable share of its jobs. \
         Give the other method its own queue, or fold the two bodies into one.",
        clashes.join(", "),
    ))
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use super::*;

    fn method(name: &'static str, queue: &'static str, retries: usize) -> ProcessMethod {
        ProcessMethod {
            name,
            queue,
            retries,
            provider_type_id: || TypeId::of::<()>(),
            handler: |_, _| Box::pin(async { Ok(()) }),
        }
    }

    #[test]
    fn one_method_per_queue_is_fine() {
        let a = method("A::a", "alpha", 1);
        let b = method("B::b", "beta", 1);
        assert!(check_duplicate_queue_claims(&[&a, &b]).is_ok());
    }

    #[test]
    fn two_methods_on_one_queue_name_both() {
        let a = method("AlphaProcessor::alpha", "transcode", 1);
        let b = method("BetaProcessor::beta", "transcode", 9);
        let err = check_duplicate_queue_claims(&[&a, &b])
            .expect_err("two claimants on one queue must not boot");
        assert!(err.contains("transcode"), "{err}");
        // Naming both is the point: the loser silently takes a share of the
        // jobs, so an error naming only one would send the reader to the wrong
        // file half the time.
        assert!(err.contains("AlphaProcessor::alpha"), "{err}");
        assert!(err.contains("BetaProcessor::beta"), "{err}");
    }
}
