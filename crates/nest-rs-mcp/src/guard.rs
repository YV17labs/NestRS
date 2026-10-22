//! A guard's refusal as the JSON-RPC error one MCP operation answers with.

use std::future::Future;
use std::pin::Pin;

use nest_rs_guards::Denial;

use crate::McpError;

/// A boxed, `Send` future — the return type of an async method in a
/// dyn-compatible trait ([`McpHost`](crate::McpHost)).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Render a guard's [`Denial`] as the JSON-RPC error one MCP operation answers
/// with.
///
/// MCP has no status line: the code picks the closest JSON-RPC family and the
/// `data` carries the machine-readable `code` and `reason`, plus
/// `requiredScopes` when the denial names them. An internal denial answers
/// opaquely ([`Opaque`](crate::Opaque)), the `INTERNAL` code its only data:
/// the reader is a language model.
pub fn denial_to_mcp_error(denial: Denial) -> McpError {
    use nest_rs_guards::__private::{client_message, structured_reason};

    if matches!(denial, Denial::Internal(_)) {
        return crate::error::opaque_answer();
    }
    let data = Some(serde_json::Value::Object(structured_reason(&denial)));
    // Not the caller's request: something the server depends on did not
    // answer, which JSON-RPC files under its internal error, with the reason
    // and the wait a client acts on.
    if matches!(denial, Denial::Unavailable { .. }) {
        return McpError::internal_error(client_message(&denial).to_owned(), data);
    }
    McpError::invalid_request(client_message(&denial).to_owned(), data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What an unavailable dependency's refusal said of it stays on the server.
    #[test]
    fn an_unavailable_denial_tells_the_client_its_condition_and_its_wait_only() {
        let error = denial_to_mcp_error(Denial::unavailable(Some(7), "store at 10.0.0.1 down"));
        let json = serde_json::to_value(&error).expect("serializes");
        assert!(!json.to_string().contains("10.0.0.1"), "{json}");
        assert_eq!(json["message"], nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE);
        assert_eq!(json["data"]["reason"], "unavailable");
        assert_eq!(json["data"]["code"], "UNAVAILABLE");
        assert_eq!(json["data"]["retryAfterSeconds"], 7);
    }

    #[test]
    fn an_internal_denial_answers_the_opaque_message() {
        let error = denial_to_mcp_error(Denial::internal("redis at 10.0.3.7 refused"));
        crate::testing::assert_opaque(&error);
    }

    /// Every denial names the code its class is known by on every edge.
    #[test]
    fn every_denial_names_the_code_of_its_class() {
        use nest_rs_guards::__private::denial_code;

        for denial in [
            Denial::unauthorized("missing bearer token"),
            Denial::invalid_credential("the token expired", "invalid_token"),
            Denial::forbidden("not yours"),
            Denial::insufficient_scope(["posts:write"], "narrow"),
            Denial::rate_limited(30, "slow down"),
            Denial::unavailable(Some(7), "store at 10.0.0.1 down"),
            Denial::internal("redis at 10.0.3.7 refused"),
        ] {
            let error = denial_to_mcp_error(denial.clone());
            assert_eq!(
                error.data.as_ref().map(|data| &data["code"]),
                Some(&serde_json::json!(denial_code(&denial).as_str())),
                "{denial:?}",
            );
        }
    }

    #[test]
    fn a_denial_names_its_class_by_the_code_a_problem_carries() {
        let error = denial_to_mcp_error(Denial::insufficient_scope(["posts:write"], "narrow"));
        let json = serde_json::to_value(&error).expect("serializes");
        assert_eq!(json["data"]["code"], "INSUFFICIENT_SCOPE");
        assert_eq!(
            json["data"]["requiredScopes"],
            serde_json::json!(["posts:write"])
        );
    }
}
