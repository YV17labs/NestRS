//! GraphQL error rendering shared by every rejection site.
//!
//! One place decides the wire shape, so the pipe path (`#[resolver]`), the
//! global-validation path (`#[crud]`) and the variable-pipe path
//! (`ContextEndpoint`) cannot drift apart — they used to, and two of the three
//! dropped the structured field errors entirely.

use async_graphql::{Error, ErrorExtensions};
use nest_rs_pipes::PipeError;

/// The extension member carrying field-level validation errors.
///
/// Deliberately the **same name** every other transport uses: an RFC 9457
/// extension member on HTTP, `data.errors` on a WebSocket frame, an `errors`
/// field on the queue's dead-letter event. A client that learned the shape on
/// one transport reads it on all of them.
pub const FIELD_ERRORS_EXTENSION: &str = "errors";

/// Render a [`PipeError`] as an `async_graphql::Error`, carrying any structured
/// field errors under `extensions.errors`.
///
/// Without this the message was the constant `"validation failed"` and there
/// were no extensions at all, so a GraphQL client could not tell *which* field
/// was wrong — while the HTTP twin, same entity and same rules, named them.
pub fn pipe_error(err: &PipeError) -> Error {
    let message = err.message().to_owned();
    match err.details() {
        // `from_json` rather than `From`: the details are `serde_json::Value`
        // and async-graphql's own `Value` has no blanket conversion. A value
        // that cannot cross (it can't — the details are plain JSON) drops the
        // extension rather than the whole error.
        Some(details) => match async_graphql::Value::from_json(details.clone()) {
            Ok(value) => {
                Error::new(message).extend_with(move |_, e| e.set(FIELD_ERRORS_EXTENSION, value))
            }
            Err(_) => Error::new(message),
        },
        // Absent, not null: a client branches on presence, exactly as on the
        // WS frame.
        None => Error::new(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extensions(err: &Error) -> serde_json::Value {
        serde_json::to_value(&err.extensions).expect("extensions serialize")
    }

    // D2: a `#[crud]` mutation rejecting an invalid input answered with
    // `{"message":"validation failed"}` and no `extensions` whatsoever, while
    // `POST /users` with the same body named both offending fields.
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
