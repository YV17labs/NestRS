//! The wrapper that carries ambient request state across rmcp's spawn.
//!
//! rmcp's [`Service`] blanket-implements over every [`ServerHandler`], and
//! `handle_request` is its single dispatch point — every tool call, resource
//! read and prompt fetch funnels through it. Wrapping that one method therefore
//! covers the whole surface without delegating rmcp's 50-odd handler methods
//! (and without breaking when it grows one).

use std::sync::Arc;

use rmcp::model::ServerResult;
use rmcp::model::{ClientNotification, ClientRequest};
use rmcp::service::{NotificationContext, RequestContext, RoleServer};
use rmcp::{ServerHandler, Service};

use crate::McpError;
use crate::context::{McpAmbient, McpToolContext, OperationOutcome};
use crate::guard::{BoxFuture, McpOperationGuard};
use crate::scope::maybe_with_request_scope;

/// Wraps a tool host so each operation runs with the request scope, the
/// operation guard's ambient state, and the registered [`McpToolContext`]'s
/// state installed.
///
/// Built by [`endpoint_with_guard`](crate::endpoint_with_guard) around the
/// handler the `#[mcp]` factory produces — never constructed by hand.
pub struct PropagatingHandler<H> {
    inner: H,
    guard: Arc<dyn McpOperationGuard>,
    context: Option<Arc<dyn McpToolContext>>,
}

impl<H> PropagatingHandler<H> {
    pub(crate) fn new(
        inner: H,
        guard: Arc<dyn McpOperationGuard>,
        context: Option<Arc<dyn McpToolContext>>,
    ) -> Self {
        Self {
            inner,
            guard,
            context,
        }
    }
}

impl<H: ServerHandler> Service<RoleServer> for PropagatingHandler<H> {
    async fn handle_request(
        &self,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<ServerResult, McpError> {
        // rmcp injects the HTTP `Parts` (extensions included) into every
        // operation's context; the endpoint stashed the ambient state there
        // before handing the request over.
        let McpAmbient {
            scope,
            captured,
            guard_captured,
        } = McpAmbient::from_extensions(&context.extensions).unwrap_or_default();

        let dispatch = self.inner.handle_request(request, context);
        let scoped: BoxFuture<'_, OperationOutcome> =
            Box::pin(maybe_with_request_scope(scope, dispatch));

        // Nesting mirrors GraphQL: the data context (which owns the operation's
        // transaction) wraps the guard's `around` (which installs the caller's
        // ability), which wraps the dispatch. With no data context registered
        // the guard still installs the ability — the tool then reads through a
        // scoped `Ability` with no executor, which is `Repo`'s fail-closed case
        // rather than a silently unscoped one.
        let guarded: BoxFuture<'_, OperationOutcome> = match &guard_captured {
            Some(captured) => self.guard.around(captured, scoped),
            None => scoped,
        };

        match (&self.context, &captured) {
            (Some(context), Some(captured)) => context.around(captured, guarded).await,
            _ => guarded.await,
        }
    }

    /// Notifications carry the same `Parts`, so the request scope is installed
    /// here too and `Scoped<T>` resolves uniformly. The [`McpToolContext`] is
    /// **not** applied: a notification is fire-and-forget with no result to
    /// commit or roll back on, so wrapping it in a transaction would invent
    /// semantics the protocol does not have. A notification handler that needs
    /// the data layer should hand the work to a request-shaped path.
    async fn handle_notification(
        &self,
        notification: ClientNotification,
        context: NotificationContext<RoleServer>,
    ) -> Result<(), McpError> {
        let scope =
            McpAmbient::from_extensions(&context.extensions).and_then(|ambient| ambient.scope);

        let dispatch = self.inner.handle_notification(notification, context);
        maybe_with_request_scope(scope, dispatch).await
    }

    fn get_info(&self) -> rmcp::model::ServerInfo {
        ServerHandler::get_info(&self.inner)
    }
}
