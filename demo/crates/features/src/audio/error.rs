//! The audio feature's typed error and its wire-facing mapping.
//!
//! Storage and queue drivers put endpoint hostnames and connection detail in
//! their error `source` chain, so the wire body must stay opaque. Each variant
//! therefore keeps a **constant** `Display` (no `#[error(transparent)]`); the
//! failing driver error rides as the `source` for `tracing`, never the wire.
//! The [`ResponseError`] impl renders the single opaque `500` and logs the full
//! chain at `error` — so a handler only has to `?`, with no per-call mapping.

use nest_rs_http::ProblemDetails;
use nest_rs_queue::QueueError;
use nest_rs_storage::StorageError;
use poem::error::ResponseError;
use poem::http::StatusCode;
use poem::{IntoResponse, Response};

/// A failure in the audio pipeline's storage or queue dependency. `Display` is
/// a constant per variant; the underlying driver error is the `source`.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// An object-storage operation (presign, put, get, head, stream) failed.
    #[error("audio storage operation failed")]
    Storage(#[from] StorageError),
    /// Enqueuing a transcode job failed.
    #[error("audio queue operation failed")]
    Queue(#[from] QueueError),
}

impl ResponseError for AudioError {
    fn status(&self) -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }

    /// The full error chain — endpoint host, driver detail — stays here for ops
    /// (`error` on `features::audio`); the client sees only the opaque constant
    /// `Display` as the problem+json `detail`.
    fn as_response(&self) -> Response {
        tracing::error!(target: "features::audio", error = ?self, "audio operation failed");
        ProblemDetails::from_status(StatusCode::INTERNAL_SERVER_ERROR)
            .with_detail(self.to_string())
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use nest_rs_queue::QueueError;

    use super::AudioError;

    // The `Storage` variant uses the byte-identical `#[error("…")]` + `#[from]`
    // form, so this proves the opaque-`Display` contract both variants share:
    // the wire (HTTP problem `detail`, and the MCP tool's mirrored constant)
    // only ever sees the fixed string, never the driver's endpoint/host detail.
    #[test]
    fn display_is_an_opaque_constant_that_hides_the_backend_detail() {
        let leaky = std::io::Error::other("connect redis://cache.internal:6379 refused");
        let err = AudioError::Queue(QueueError::backend(leaky));

        assert_eq!(err.to_string(), "audio queue operation failed");
        assert!(
            !err.to_string().contains("cache.internal"),
            "the backend host must never reach the wire: {err}",
        );
        // The driver detail is retained as the source, for `tracing` only.
        assert!(
            err.source().is_some(),
            "the underlying error stays as the source for observability",
        );
    }
}
