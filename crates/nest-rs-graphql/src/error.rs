//! GraphQL error rendering shared by every rejection site.

use async_graphql::{Error, ErrorExtensions};
use nest_rs_pipes::PipeError;

/// The extension member carrying field-level validation errors — the name
/// every other transport uses for them.
pub const FIELD_ERRORS_EXTENSION: &str = "errors";

/// Render a [`PipeError`] as an `async_graphql::Error`, carrying any structured
/// field errors under `extensions.errors`.
pub fn pipe_error(err: &PipeError) -> Error {
    let message = err.message().to_owned();
    match err.details() {
        // Plain JSON always converts; a failure would drop the extension, not the error.
        Some(details) => match async_graphql::Value::from_json(details.clone()) {
            Ok(value) => {
                Error::new(message).extend_with(move |_, e| e.set(FIELD_ERRORS_EXTENSION, value))
            }
            Err(_) => Error::new(message),
        },
        // Absent, not null: a client branches on presence.
        None => Error::new(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extensions(err: &Error) -> serde_json::Value {
        serde_json::to_value(&err.extensions).expect("extensions serialize")
    }

    #[test]
    fn a_rejection_with_details_carries_them_under_the_errors_extension() {
        let details = serde_json::json!({
            "name": [{ "code": "length", "params": { "min": 1 } }],
            "email": [{ "code": "email" }],
        });
        let err = pipe_error(&PipeError::with_details(
            "validation failed",
            details.clone(),
        ));

        assert_eq!(err.message, "validation failed");
        assert_eq!(extensions(&err)[FIELD_ERRORS_EXTENSION], details);
    }

    #[test]
    fn a_rejection_without_details_carries_no_extensions() {
        let err = pipe_error(&PipeError::new("must be a valid UUID"));
        assert_eq!(err.message, "must be a valid UUID");
        assert!(
            err.extensions.is_none(),
            "absent, not null — a client branches on presence",
        );
    }
}
