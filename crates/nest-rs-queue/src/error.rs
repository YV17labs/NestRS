//! Typed errors for the queue port: [`QueueError`], what a producer gets back,
//! and [`JobError`], what a job attempt reports to its backend.
//!
//! Framework crates surface `thiserror` enums, not `anyhow`. An enqueue can
//! fail two ways: serializing the job to its JSON wire form, or inside the
//! backend's push. The backend failure is kept behind a boxed `source` so this
//! contract names no concrete backend — a Redis backend wraps its apalis/Redis
//! error, an SQS backend its SDK error, without this crate depending on either.

use thiserror::Error;

/// A failure enqueuing a job through a [`JobProducer`](crate::JobProducer) (or
/// the [`push`](crate::JobProducerExt::push) convenience over it).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum QueueError {
    /// The job could not be serialized to its JSON wire form.
    #[error("failed to serialize job payload")]
    Serialize(#[from] serde_json::Error),
    /// The backend rejected or failed the enqueue. The concrete backend
    /// failure is the `source`, kept opaque so the producer contract stays
    /// backend-agnostic.
    #[error("queue backend failed to enqueue job")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl QueueError {
    /// Wrap a backend-specific enqueue failure as [`QueueError::Backend`]. A
    /// backend calls this to surface its concrete error (an apalis/Redis error,
    /// an SQS SDK error, …) without this crate naming the type — e.g.
    /// `storage.push(job).await.map_err(QueueError::backend)?`.
    pub fn backend<E>(source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Backend(Box::new(source))
    }
}

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
