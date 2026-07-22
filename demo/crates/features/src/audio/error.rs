use nest_rs_http::ProblemDetails;
use nest_rs_queue::QueueError;
use nest_rs_storage::StorageError;
use poem::error::ResponseError;
use poem::http::StatusCode;
use poem::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("audio storage operation failed")]
    Storage(#[from] StorageError),
    #[error("audio queue operation failed")]
    Queue(#[from] QueueError),
}

impl ResponseError for AudioError {
    fn status(&self) -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }

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

    #[test]
    fn display_is_an_opaque_constant_that_hides_the_backend_detail() {
        let leaky = std::io::Error::other("connect redis://cache.internal:6379 refused");
        let err = AudioError::Queue(QueueError::backend(leaky));

        assert_eq!(err.to_string(), "audio queue operation failed");
        assert!(
            !err.to_string().contains("cache.internal"),
            "the backend host must never reach the wire: {err}",
        );
        assert!(
            err.source().is_some(),
            "the underlying error stays as the source for observability",
        );
    }
}
