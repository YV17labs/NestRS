//! Wire-facing failure mapping for the audio HTTP surface.
//!
//! Storage and queue drivers put endpoint hostnames and connection detail in
//! their `Display` output, so the response body must stay opaque. The
//! framework already owns that contract — [`ServiceError::Internal`] renders
//! the constant `"internal error"` problem+json `500` — so these helpers only
//! add the boundary log (full chain at `error`, with the failing operation as
//! a structured field) before delegating to it.

use nest_rs_seaorm::ServiceError;

pub(crate) fn storage_error(op: &'static str, source: anyhow::Error) -> ServiceError {
    tracing::error!(target: "features::audio", op, error = ?source, "storage operation failed");
    ServiceError::internal(format!("audio storage operation failed: {op}"))
}

pub(crate) fn queue_error(op: &'static str, source: nest_rs_queue::QueueError) -> ServiceError {
    tracing::error!(target: "features::audio", op, error = ?source, "queue operation failed");
    ServiceError::internal(format!("audio queue operation failed: {op}"))
}
