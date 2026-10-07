//! Typed errors for the queue port: [`QueueError`], what a caller of the port
//! gets back — a push, a cancel, a checkpoint save — and [`JobError`], what a
//! job attempt reports to its backend.
//!
//! Framework crates surface `thiserror` enums, not `anyhow`. A backend failure
//! is kept behind a boxed `source`, so this contract names no backend — a Redis
//! backend wraps its storage error, an SQS backend its SDK error, without this
//! crate depending on either.

use thiserror::Error;

use crate::capability::unsupported;
use crate::{BACKEND_TIMEOUT, Capability, JobId, PushOptions, PushReceipt, QueueName};

/// A failure of a queue operation: a push, a cancel, or a checkpoint save.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum QueueError {
    /// A job, or a checkpoint's state, could not be converted to or from JSON.
    #[error("failed to convert the queue value to or from JSON")]
    Serialize(#[from] serde_json::Error),
    /// The backend rejected or failed the operation. Its own failure is the
    /// `source`, kept opaque so the port names no backend.
    #[error("the queue backend failed")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
    /// The backend did not answer a call within [`BACKEND_TIMEOUT`], and the
    /// port stopped waiting.
    ///
    /// Not answering is not refusing: the backend may still carry the call out
    /// once it answers again — a push then files its jobs, which run; a cancel
    /// then cancels. So a caller that pushes again after this error enqueues
    /// the job twice, unless the push carries a unique key — the same as after
    /// any failure to answer.
    #[error(
        "the queue backend did not answer `{call}` on queue `{queue}` within {timeout:?}, and the \
         port stopped waiting — the backend may still carry the call out once it answers",
        timeout = BACKEND_TIMEOUT,
    )]
    Unanswered {
        /// The queue the call was made for.
        queue: QueueName,
        /// The backend method that did not answer, named by its trait —
        /// `JobProducer::enqueue`, `CheckpointStore::save`.
        call: &'static str,
    },
    /// The operation needs a capability its backend does not declare, and was
    /// refused before the backend saw it.
    #[error("{}", unsupported(*capability, backend, None))]
    Unsupported {
        /// What the operation needs.
        capability: Capability,
        /// The backend that does not provide it.
        backend: &'static str,
    },
    /// A queue name outside the rule [`QueueName`] states.
    #[error(
        "{name:?} is not a valid queue name: it takes 1 to {max} ASCII letters, digits, `_`, `.` \
         or `-` — `:` separates the levels of a backend's keys, and whitespace would reach a log \
         field or a metric label",
        max = QueueName::MAX_LEN,
    )]
    InvalidQueueName {
        /// The value refused, truncated to [`QueueName::MAX_LEN`] characters.
        name: String,
    },
    /// A unique key no backend could enqueue.
    #[error(
        "invalid unique key: {reason} — a unique key is 1 to {max} bytes with no control character",
        max = PushOptions::MAX_UNIQUE_KEY_LEN,
    )]
    InvalidUniqueKey {
        /// Which part of the rule it broke.
        reason: &'static str,
    },
    /// A job id no push could have minted — see [`JobId::parse`].
    #[error("{id:?} is not a job id: a job id is a UUID v7, as a push mints it")]
    InvalidJobId {
        /// The value refused, truncated to [`QueueError::SHOWN_ID_LEN`]
        /// characters.
        id: String,
    },
    /// A push under a unique key another job on the same queue still holds.
    ///
    /// The push filed nothing. The key is held while its job is pending or
    /// running, and is free again once that job completes, dead-letters or is
    /// cancelled — [`cancel_unique`](crate::JobProducerExt::cancel_unique)
    /// frees it while the job waits.
    #[error(
        "unique key {key:?} on queue `{queue}` is held by job `{holder}`, which is still pending \
         or running — the push was refused and filed nothing"
    )]
    UniqueKeyHeld {
        /// The queue the push named.
        queue: QueueName,
        /// The key the push declared.
        key: String,
        /// The job holding it.
        holder: JobId,
    },
    /// A push of many failed after the backend accepted some of its calls.
    ///
    /// `receipts` names the jobs of every call the backend accepted, in input
    /// order — they are queued and will run, and a receipt is what cancels one
    /// or tells it from a job to push again. `source` is the failure of the call
    /// that stopped the push, whose own jobs no receipt covers: a backend's call
    /// is not atomic, so some of them may be queued too — at-least-once, as after
    /// any failure to answer. A push failing at its first call returns that
    /// failure itself.
    #[error(
        "the queue backend accepted {} job(s) of the push before a call failed; their receipts \
         are on the error, and the jobs of the failed call may be queued too",
        receipts.len(),
    )]
    PartiallyQueued {
        /// The jobs the backend accepted, in input order.
        receipts: Vec<PushReceipt>,
        /// Why the push stopped.
        #[source]
        source: Box<QueueError>,
    },
    /// Options no push could honour: together, or at all on this backend.
    ///
    /// Both readings are here on purpose. The port refuses the combinations
    /// nothing could honour — a unique key on `push_many` — and a backend
    /// refuses a single value its own storage cannot represent, naming that
    /// fact (`the delay ends past what the clock can represent`). A caller
    /// matching on this variant is told the options were refused and given the
    /// reason; which of the two it was is the `reason`, never the variant.
    #[error("invalid push options: {reason}")]
    InvalidOptions {
        /// Why they were refused.
        reason: &'static str,
    },
}

impl QueueError {
    /// How much of a refused job id an error shows: a UUID's hyphenated form
    /// and then some, so a near-miss is recognisable and a pasted blob is not
    /// carried into every log line that renders the error.
    pub const SHOWN_ID_LEN: usize = 64;

    /// Wrap a backend's own failure as [`QueueError::Backend`], without this
    /// crate naming its type — `storage.push(job).await.map_err(QueueError::backend)?`.
    pub fn backend<E>(source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Backend(Box::new(source))
    }
}

/// A job failure classified for the retry budget.
///
/// A **retryable** failure ([`retry`](JobError::retry)) is a transient fault —
/// the `#[process]` method returning `Err` — that another attempt might clear.
/// A **non-retryable** failure ([`abort`](JobError::abort)) is *deterministic* —
/// an unsupported wire-format version, an undeserializable payload, a missing
/// provider — and another attempt would fail identically, so the attempt
/// dead-letters at once instead of spending the budget on it.
pub struct JobError {
    /// Whether another attempt could clear the failure.
    pub retryable: bool,
    /// The underlying error, for the log and the backend's dead-letter record.
    pub source: Box<dyn std::error::Error + Send + Sync>,
    /// Structured detail the failure carried, when it had any — the per-field
    /// errors of a `Valid<T>` job-argument rejection.
    ///
    /// A dead-lettered job is read from a log, days later, by someone who cannot
    /// re-run it: `error=validation failed` alone does not say which field of
    /// which payload was wrong, and the information existed at the moment of
    /// failure. It rides the dead-letter event beside the error, under the same
    /// `errors` name HTTP and WebSockets use.
    pub details: Option<serde_json::Value>,
}

impl JobError {
    /// A **retryable** failure: a transient fault worth another attempt.
    ///
    /// Boxed by [`nest_rs_core::boxed_error`], so an `anyhow::Error` — what a
    /// `#[process]` method returns — keeps every link of its chain readable, and
    /// a decode failure inside it reaches the line and the dead-letter record as
    /// its report rather than as serde's sentence quoting the value.
    pub fn retry(source: impl Into<Box<dyn std::error::Error + Send + Sync>> + 'static) -> Self {
        Self {
            retryable: true,
            source: nest_rs_core::boxed_error(source),
            details: None,
        }
    }

    /// A **non-retryable** failure: deterministic, so another attempt would fail
    /// identically — the attempt dead-letters at once. Boxed as
    /// [`retry`](Self::retry) boxes.
    pub fn abort(source: impl Into<Box<dyn std::error::Error + Send + Sync>> + 'static) -> Self {
        Self {
            retryable: false,
            source: nest_rs_core::boxed_error(source),
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

    /// The failure of a payload that does not decode as the job type of the
    /// queue it came from — deterministic, since the same bytes never decode on
    /// a retry. Said by where and what kind ([`DecodeError`](nest_rs_core::DecodeError)),
    /// never by the value: the sentence lands in the dead-letter log line and
    /// record, and the payload is somebody's data.
    ///
    /// `#[doc(hidden)]`: the handler `#[processor]` emits is its one caller.
    #[doc(hidden)]
    pub fn undecodable(queue: &str, error: &serde_json::Error) -> Self {
        Self::abort(format!(
            "failed to deserialize job for queue `{queue}`: {}",
            nest_rs_core::DecodeError::new(error),
        ))
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

/// A backend declaring job cancellation without implementing the removal it
/// needs — a driver defect, reported as one.
#[derive(Debug, thiserror::Error)]
#[error(
    "the `{backend}` queue backend declares job cancellation and does not implement \
     `JobProducer::{method}` — a backend implements what each capability it declares needs"
)]
pub(crate) struct Unimplemented {
    pub(crate) backend: &'static str,
    pub(crate) method: &'static str,
}

/// A call the worker made to its backend that did not answer `Ok`: the
/// backend's own error, or no answer within the port's net.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CallFailed {
    #[error("{}", nest_rs_core::error_message(.0))]
    Erred(QueueError),
    #[error("no answer within the port's net of {0:?}")]
    Unanswered(std::time::Duration),
}
