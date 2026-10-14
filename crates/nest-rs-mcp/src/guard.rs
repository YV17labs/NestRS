//! Per-operation guard the MCP endpoint runs before each streamable-HTTP request.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;

use nest_rs_guards::Denial;
use poem::{Request, Result};

use crate::McpError;
use crate::context::{Captured, OperationOutcome};

/// A boxed, `Send` future — the return type of an async guard method in a
/// dyn-compatible trait.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Authenticates an MCP HTTP request before the streamable handler runs. Bind
/// with `providers = [MyBridge as dyn McpOperationGuard]`.
///
/// A registered guard adds to the global guard pool, which always runs at the
/// request. It gates the HTTP request, and never stands in for an operation's
/// own `Guard::check` chain.
pub trait McpOperationGuard: Send + Sync + 'static {
    /// Gate the operation: check `req` — keeping what the checks attach on it,
    /// as `nest_rs_http`'s request guard stage does — and return `Err` to
    /// reject it before the handler runs.
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, Result<()>>;

    /// Snapshot what [`around`](Self::around) will need, from the post-`before`
    /// request. `None` (the default) means this guard installs nothing and
    /// `around` is never called for it.
    fn capture(&self, _req: &Request) -> Option<Captured> {
        None
    }

    /// Wrap one operation's dispatch to install ambient state for its duration
    /// (the caller's `Ability`), inside rmcp's spawned dispatch, on whatever
    /// [`capture`](Self::capture) returned. Default = pass-through.
    fn around<'a>(
        &'a self,
        _captured: &'a Captured,
        inner: BoxFuture<'a, OperationOutcome>,
    ) -> BoxFuture<'a, OperationOutcome> {
        inner
    }

    /// The guards [`before`](Self::before) runs itself, by type. The global pool
    /// leaves each of them to this guard, so a guard listed in both runs once.
    fn covers(&self) -> &[TypeId] {
        &[]
    }
}

/// Render a guard's [`Denial`] as the JSON-RPC error one MCP operation answers
/// with.
///
/// MCP has no status line: the code picks the closest JSON-RPC family and the
/// `data` carries the machine-readable `code` and `reason`, plus
/// `requiredScopes` when the denial names them. An internal denial is opaque
/// ([`Opaque`](crate::Opaque)): the reader is a language model.
pub fn denial_to_mcp_error(denial: Denial) -> McpError {
    use nest_rs_guards::__private::{client_message, structured_reason};

    if matches!(denial, Denial::Internal(_)) {
        return McpError::internal_error(nest_rs_core::OPAQUE_CLIENT_MESSAGE, None);
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
        assert_eq!(error.message, nest_rs_core::OPAQUE_CLIENT_MESSAGE);
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
