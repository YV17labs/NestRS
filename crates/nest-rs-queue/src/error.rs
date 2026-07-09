//! Typed errors for the queue producer surface.
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
