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
