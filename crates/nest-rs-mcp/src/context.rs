//! Per-operation ambient-state bridge for MCP handler bodies — the MCP mirror
//! of `nest-rs-ws`'s `SocketContext`.
//!
//! rmcp dispatches every operation on its own spawned task, so a task-local
//! installed around the poem endpoint is gone by the time a handler runs. rmcp
//! does, however, inject the request's [`Parts`](poem::http::request::Parts) —
//! extensions included — into each operation's `RequestContext`. That is the
//! carrier this module rides: the endpoint stashes an [`McpAmbient`] in the
//! request extensions, and [`PropagatingHandler`](crate::PropagatingHandler)
//! reads it back **inside** the spawned dispatch and re-installs everything
//! around the call.
//!
//! Implement [`McpToolContext`] to re-install a data context (the ORM executor
//! and authz ability); `nest_rs_seaorm::mcp::McpDataContext` is the first-party one.
//! List it `as dyn McpToolContext` on the tool host's module.

use std::any::{Any, type_name};
use std::sync::Arc;

use nest_rs_core::RequestScope;
use poem::Request;
use poem::http::request::Parts;
use rmcp::model::Extensions;

use crate::McpError;
use crate::guard::BoxFuture;

/// Opaque state a [`McpToolContext`] captures on the HTTP request and reads
/// back inside the tool dispatch. Downcast it to your own type in
/// [`around`](McpToolContext::around).
pub type Captured = Arc<dyn Any + Send + Sync>;

/// The success value of one MCP operation, type-erased.
///
/// Every rmcp `ServerHandler` method has its own result type — `CallToolResponse`,
/// `GetPromptResponse`, `ListResourcesResult`, `GetTaskResult`, … — but the
/// [`McpToolContext`] and [`McpOperationGuard`](crate::McpOperationGuard) seams
/// are `dyn`, so they cannot be generic over it. Erasing the value is what lets
/// **one** `around` implementation wrap **every** MCP capability instead of the
/// tool call alone; a wrapper only ever inspects `Ok`/`Err` (commit vs
/// rollback), never the value itself.
pub struct OperationValue(Box<dyn Any + Send>);

impl OperationValue {
    pub(crate) fn new<T: Send + 'static>(value: T) -> Self {
        Self(Box::new(value))
    }

    /// Recover the concrete result. A miss means an `around` implementation
    /// substituted a value of a different type, which is a framework bug in
    /// that implementation — reported as an opaque internal error rather than a
    /// panic on the dispatch path.
    pub(crate) fn take<T: Send + 'static>(self) -> Result<T, McpError> {
        match self.0.downcast::<T>() {
            Ok(value) => Ok(*value),
            Err(_) => {
                tracing::error!(
                    target: "nest_rs::mcp",
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
/// [`capture`](Self::capture) runs on the poem request, after the guard chain
/// and while the ambient executor / ability are still reachable;
/// [`around`](Self::around) runs inside rmcp's spawned dispatch, where they are
/// not. Splitting the two is what carries request state across the spawn.
pub trait McpToolContext: Send + Sync + 'static {
    /// Snapshot what the operation will need, from the post-guard request.
    fn capture(&self, req: &Request) -> Captured;

    /// Wrap one *request* operation with the captured state installed — every
    /// MCP capability the server answers: `tools/call`, `prompts/get`,
    /// `resources/read`, `completion/complete`, `logging/setLevel`, the
    /// `tasks/*` trio, and any custom method.
    ///
    /// Two kinds of operation are excluded, both on purpose. **Notifications**
    /// are fire-and-forget: there is no outcome to commit or roll back on.
    /// **`subscriptions/listen`** runs for the lifetime of the subscription, so
    /// holding a transaction open across it would leak a connection; it gets
    /// the request scope and the guard's ability, and a handler that needs the
    /// data layer hands the work to a request-shaped path.
    fn around<'a>(
        &'a self,
        captured: &'a Captured,
        inner: BoxFuture<'a, OperationOutcome>,
    ) -> BoxFuture<'a, OperationOutcome>;
}

/// The value the endpoint puts in the HTTP request extensions so it survives
/// into rmcp's per-operation `RequestContext`. Cheap to clone (three `Arc`s).
#[derive(Clone, Default)]
pub(crate) struct McpAmbient {
    /// The per-request scope backing [`Scoped<T>`](crate::Scoped).
    pub(crate) scope: Option<Arc<RequestScope>>,
    /// Whatever the registered [`McpToolContext`] snapshotted.
    pub(crate) captured: Option<Captured>,
    /// Whatever the endpoint's
    /// [`McpOperationGuard`](crate::McpOperationGuard) snapshotted — the
    /// caller's ability, for the canonical bridge.
    pub(crate) guard_captured: Option<Captured>,
}

impl McpAmbient {
    /// Read the ambient state back out of the `Parts` rmcp injects into every
    /// operation's context. Both dispatch paths (`handle_request` and
    /// `handle_notification`) go through here, so the carrier's shape is named
    /// once.
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

    /// `around` is handed the operation as an opaque value and hands one back.
    /// An implementation that substitutes a value of a different type — rather
    /// than returning the one it was given — leaves the dispatch with something
    /// it cannot hand to rmcp.
    ///
    /// Answering with an opaque internal error rather than panicking is what
    /// keeps one misbehaving wrapper from taking the endpoint down; and since
    /// that error is indistinguishable from any other, the event is the only
    /// place the substitution is recorded. It carries the type that *was*
    /// expected, because the wrapper's own name is not on the dispatch path.
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
