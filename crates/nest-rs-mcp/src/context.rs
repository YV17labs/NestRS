//! Per-operation ambient-state bridge for MCP handler bodies.
//!
//! rmcp dispatches every operation on its own task, where a task-local installed
//! around the poem endpoint is gone, but it injects the request's [`Parts`] into
//! each operation's `RequestContext`: the endpoint stashes an [`McpAmbient`] in
//! their extensions, and [`PropagatingHandler`](crate::PropagatingHandler)
//! re-installs it inside the dispatch.

use std::any::{Any, type_name};
use std::sync::Arc;

use nest_rs_core::RequestScope;
use poem::Request;
use poem::http::request::Parts;
use rmcp::model::Extensions;

use crate::McpError;
use crate::guard::BoxFuture;

/// Opaque state a [`McpToolContext`] captures on the HTTP request; downcast it
/// to your own type in [`around`](McpToolContext::around).
pub type Captured = Arc<dyn Any + Send + Sync>;

/// The success value of one MCP operation, type-erased so one `dyn` `around`
/// wraps every capability; a wrapper inspects `Ok`/`Err`, never the value.
pub struct OperationValue(Box<dyn Any + Send>);

impl OperationValue {
    pub(crate) fn new<T: Send + 'static>(value: T) -> Self {
        Self(Box::new(value))
    }

    /// Recover the concrete result; a miss (an `around` substituted another type)
    /// is an opaque internal error, never a panic on the dispatch path.
    pub(crate) fn take<T: Send + 'static>(self) -> Result<T, McpError> {
        match self.0.downcast::<T>() {
            Ok(value) => Ok(*value),
            Err(_) => {
                tracing::error!(
                    target: crate::TARGET,
                    expected = type_name::<T>(),
                    reason = "operation_value_downcast_miss",
                    "mcp operation wrapper returned a foreign value",
                );
                Err(McpError::internal_error("internal error".to_string(), None))
            }
        }
    }
}

/// What one MCP operation resolves to — the unit a [`McpToolContext`] wraps.
pub type OperationOutcome = Result<OperationValue, McpError>;

/// Re-installs ambient per-request state around each MCP operation.
///
/// [`capture`](Self::capture) runs on the poem request, after the guard chain,
/// while the ambient executor and ability are reachable; [`around`](Self::around)
/// runs inside rmcp's spawned dispatch, where they are not.
pub trait McpToolContext: Send + Sync + 'static {
    /// Snapshot what the operation will need, from the post-guard request.
    fn capture(&self, req: &Request) -> Captured;

    /// Wrap one *request* operation — every capability the server answers — with
    /// the captured state installed.
    ///
    /// Notifications are not wrapped, having no outcome to commit, nor
    /// `subscriptions/listen`, whose transaction would stay open for the
    /// subscription's lifetime; it gets the request scope and the guard's ability.
    fn around<'a>(
        &'a self,
        captured: &'a Captured,
        inner: BoxFuture<'a, OperationOutcome>,
    ) -> BoxFuture<'a, OperationOutcome>;
}

/// The value the endpoint puts in the HTTP request extensions so it survives
/// into rmcp's per-operation `RequestContext`.
#[derive(Clone)]
pub(crate) struct McpAmbient {
    /// The per-request scope backing [`Scoped<T>`](crate::Scoped).
    pub(crate) scope: Option<Arc<RequestScope>>,
    /// Whatever the registered [`McpToolContext`] snapshotted.
    pub(crate) captured: Option<Captured>,
    /// Whatever the endpoint's [`McpOperationGuard`](crate::McpOperationGuard)
    /// snapshotted — the caller's ability, for the canonical bridge.
    pub(crate) guard_captured: Option<Captured>,
    /// The span the request arrived under: in session mode rmcp's serve loop runs
    /// on a bare `tokio::spawn`, so a tool's events would otherwise lose the request.
    pub(crate) span: tracing::Span,
    /// The HTTP request's id and resolved actor, as the access log filed them.
    pub(crate) correlation: nest_rs_core::Correlation,
}

impl Default for McpAmbient {
    /// A disabled span, and a minted id so an operation with no HTTP edge still
    /// groups its events.
    fn default() -> Self {
        Self {
            scope: None,
            captured: None,
            guard_captured: None,
            span: tracing::Span::none(),
            correlation: nest_rs_core::Correlation::minted(None),
        }
    }
}

impl McpAmbient {
    /// Read the ambient state back out of the `Parts` rmcp injects into every
    /// operation's context.
    pub(crate) fn from_extensions(extensions: &Extensions) -> Option<Self> {
        extensions
            .get::<Parts>()
            .and_then(|parts| parts.extensions.get::<Self>())
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The event is the only record of the substitution, so it names the type
    /// expected; the wrapper's own name is not on the dispatch path.
    #[test]
    fn an_operation_value_of_the_wrong_type_is_an_opaque_error_and_a_named_event() {
        let logs = nest_rs_testing::LogCapture::install();
        let substituted = OperationValue::new(42u8);

        let err = substituted
            .take::<String>()
            .expect_err("a value of another type cannot be handed back as this one");
        assert!(
            !err.message.contains("u8") && !err.message.contains("String"),
            "the client — a language model — learns nothing about the internals: {}",
            err.message,
        );

        let event = logs.expect_one(
            "nest_rs::mcp",
            "mcp operation wrapper returned a foreign value",
        );
        assert_eq!(event.level, "error");
        assert!(
            event
                .field("expected")
                .is_some_and(|e| e.contains("String")),
            "the event names the type the dispatch was waiting for, got {:?}",
            event.fields,
        );
        assert_eq!(
            event.field("reason").as_deref(),
            Some("operation_value_downcast_miss"),
            "{:?}",
            event.fields,
        );
    }

    #[test]
    fn an_operation_value_of_the_right_type_comes_back_untouched() {
        let logs = nest_rs_testing::LogCapture::install();
        assert_eq!(
            OperationValue::new(String::from("answer"))
                .take::<String>()
                .expect("the value the wrapper was given"),
            "answer",
        );
        logs.expect_none(
            "nest_rs::mcp",
            "mcp operation wrapper returned a foreign value",
        );
    }
}
