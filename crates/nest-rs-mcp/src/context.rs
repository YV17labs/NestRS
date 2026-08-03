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
