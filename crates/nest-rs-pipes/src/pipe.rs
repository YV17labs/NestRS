/// A pipe `transform`s an extracted value into a new value or a [`PipeError`].
///
/// Pipes are **stateless** — a zero-sized marker named at a call site
/// (`Piped<ParseInt, _>`), never instantiated — so `transform` is an associated
/// function. Stateful/DI-injected pipes would need a different binding.
pub trait Pipe {
    type In;
    type Out;
    fn transform(input: Self::In) -> Result<Self::Out, PipeError>;
}

/// Why a pipe rejected its input. A surface adapter renders it (the HTTP one as
/// a `400`). Carries a human `message` plus optional structured `details` (e.g.
/// the field-level errors from [`ValidationPipe`](crate::ValidationPipe)).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct PipeError {
    message: String,
    details: Option<serde_json::Value>,
}

impl PipeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(message: impl Into<String>, details: serde_json::Value) -> Self {
        Self {
            message: message.into(),
            details: Some(details),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn details(&self) -> Option<&serde_json::Value> {
        self.details.as_ref()
    }

    pub fn into_details(self) -> Option<serde_json::Value> {
        self.details
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_omits_details() {
        let err = PipeError::new("not a uuid");
        assert_eq!(err.message(), "not a uuid");
        assert!(err.details().is_none());
    }

    #[test]
    fn display_matches_message() {
        // `PipeError`'s `#[error("{message}")]` discipline matters — a renderer
        // (HTTP, WS) writes `Display`, never the inner struct.
        let err = PipeError::new("not a uuid");
        assert_eq!(err.to_string(), "not a uuid");
    }

    #[test]
    fn with_details_round_trips_payload() {
        let payload = serde_json::json!({ "field": ["bad"] });
        let err = PipeError::with_details("validation failed", payload.clone());
        assert_eq!(err.message(), "validation failed");
        assert_eq!(err.details(), Some(&payload));
    }

    #[test]
    fn into_details_consumes_and_returns_value() {
        let err = PipeError::with_details("x", serde_json::json!({"k": 1}));
        let details = err.into_details().expect("details");
        assert_eq!(details["k"], 1);
    }

    #[test]
    fn into_details_on_a_plain_error_returns_none() {
        assert!(PipeError::new("plain").into_details().is_none());
    }
}
