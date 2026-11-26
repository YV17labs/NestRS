use nest_rs_core::{Problem, ToProblem, code};

/// Why a pipe rejected its input: a human `message` plus optional structured
/// `details` (e.g. the field-level errors of a type's `validator` rules,
/// `From` its `ValidationErrors`).
///
/// It carries the [`Problem`] every edge answers it as — `400` under
/// `INVALID_ARGUMENT`, the message its detail and the details its `errors`
/// field map — as its source, so a refusal answers the same wherever it was
/// raised: by a per-argument pipe, or returned from a handler's body, bare or
/// wrapped in the handler's own error.
///
/// ```
/// use nest_rs_core::{ToProblem, code};
/// use nest_rs_pipes::PipeError;
///
/// let refused = PipeError::new("must be a valid UUID");
/// let problem = refused.to_problem().expect("a refusal answers its problem");
/// assert_eq!((problem.status(), problem.code()), (400, code::INVALID_ARGUMENT));
/// assert_eq!(problem.detail(), Some("must be a valid UUID"));
/// ```
#[derive(Debug, Clone, thiserror::Error)]
#[error("{}", .problem.detail().unwrap_or_default())]
pub struct PipeError {
    #[source]
    problem: Problem,
}

impl PipeError {
    /// A rejection carrying just a human-readable message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            problem: Problem::new(400, code::INVALID_ARGUMENT).with_detail(message.into()),
        }
    }

    /// A rejection with a message plus structured `details` (e.g. field errors)
    /// the surface adapter can render.
    pub fn with_details(message: impl Into<String>, details: serde_json::Value) -> Self {
        Self {
            problem: Self::new(message).problem.with_errors(details),
        }
    }

    /// The human-readable rejection message.
    pub fn message(&self) -> &str {
        self.problem.detail().unwrap_or_default()
    }

    /// The structured details, if any were attached.
    pub fn details(&self) -> Option<&serde_json::Value> {
        self.problem.errors()
    }

    /// The structured details as an owned value, if any were attached.
    pub fn into_details(self) -> Option<serde_json::Value> {
        self.problem.errors().cloned()
    }

    /// The problem every edge answers the rejection as.
    pub fn problem(&self) -> &Problem {
        &self.problem
    }
}

/// A rejection returned as a handler's error answers its problem.
impl ToProblem for PipeError {
    fn to_problem(&self) -> Option<Problem> {
        Some(self.problem.clone())
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
        // A renderer (HTTP, WS) writes `Display`, never the inner struct.
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

    #[test]
    fn a_rejection_is_found_in_a_chain_as_the_problem_it_answers() {
        use std::error::Error as _;

        let refused = PipeError::with_details("validation failed", serde_json::json!({"k": 1}));
        let problem = refused
            .source()
            .and_then(|source| source.downcast_ref::<Problem>())
            .expect("the problem is the rejection's source");
        assert_eq!(problem, refused.problem());
        assert_eq!(
            (problem.status(), problem.code()),
            (400, code::INVALID_ARGUMENT)
        );
        assert_eq!(problem.detail(), Some("validation failed"));
        assert_eq!(problem.errors(), Some(&serde_json::json!({"k": 1})));
        assert_eq!(refused.to_problem().as_ref(), Some(problem));
    }
}
