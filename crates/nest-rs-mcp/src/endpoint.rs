//! The poem endpoint that serves an MCP handler over streamable HTTP.

use std::sync::Arc;

use nest_rs_core::{Container, RequestScope};
use poem::endpoint::TowerCompatExt;
use poem::{Endpoint, IntoEndpoint, Request, Response, Result, Route};
use rmcp::ServerHandler;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use crate::context::{McpAmbient, McpToolContext};
use crate::guard::{FallbackMcpGuard, McpOperationGuard};
use crate::guards::deny_all;
use crate::propagate::PropagatingHandler;

/// The operation guard an MCP mount runs, in preference order: the app's
/// registered `dyn McpOperationGuard` (the authz bridge), else the global guard
/// pool through the seeded [`FallbackMcpGuard`], else deny-all.
///
/// This is the order the `#[mcp]` macro mounts with, and the MCP twin of what
/// `ContextEndpoint::new` does for `/graphql`. Keeping deny-all as the tail
/// means the fallback only ever widens what `use_guards_global` opted into —
/// an app with no guards at all still gets a closed tool surface.
pub fn resolve_operation_guard(container: &Container) -> Arc<dyn McpOperationGuard> {
    let (guard, mode) = match container.get_dyn::<dyn McpOperationGuard>() {
        Some(guard) => (guard, "operation_guard"),
        None => match container.get::<FallbackMcpGuard>() {
            Some(fallback) => ((fallback.0)(container), "global_guard_pool"),
            // `deny_all` says the fail-closed posture itself, at `warn`.
            None => return deny_all(),
        },
    };
    tracing::debug!(target: "nest_rs::mcp", mode, "mcp operations gated");
    guard
}

/// `factory` runs on every new MCP session, so per-session state stays fresh.
pub fn endpoint<F, H>(factory: F) -> impl IntoEndpoint
where
    F: Fn() -> H + Send + Sync + 'static,
    H: ServerHandler + Send + 'static,
{
    // Fail closed: an MCP endpoint mounted without an explicit
    // `McpOperationGuard` denies every request rather than serving the tool
    // surface unauthenticated.
    endpoint_with_guard(deny_all(), None, factory)
}

/// Like [`endpoint`], but runs `guard` before each operation and re-installs
/// `context`'s ambient state around each one. `None` for the context means tool
/// bodies get the request scope but no data context (`Repo` then fails closed).
///
/// The guard is not optional: every mount path picks one first — [`endpoint`]
/// uses deny-all, the `#[mcp]` macro uses [`resolve_operation_guard`], whose own
/// tail is deny-all — so the fail-closed default is stated once, at the point
/// that chooses it, rather than re-defaulted here.
pub fn endpoint_with_guard<F, H>(
    guard: Arc<dyn McpOperationGuard>,
    context: Option<Arc<dyn McpToolContext>>,
    factory: F,
) -> impl IntoEndpoint
where
    F: Fn() -> H + Send + Sync + 'static,
    H: ServerHandler + Send + 'static,
{
    let handler_context = context.clone();
    let handler_guard = guard.clone();
    let service = StreamableHttpService::new(
        move || {
            Ok(PropagatingHandler::new(
                factory(),
                handler_guard.clone(),
                handler_context.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let inner = service.compat();
    Route::new().at(
        "/",
        GuardedEndpoint {
            guard,
            context,
            inner,
        },
    )
}

struct GuardedEndpoint<E> {
    guard: Arc<dyn McpOperationGuard>,
    context: Option<Arc<dyn McpToolContext>>,
    inner: E,
}

impl<E> Endpoint for GuardedEndpoint<E>
where
    E: Endpoint<Output = Response>,
{
    type Output = Response;

    async fn call(&self, mut req: Request) -> Result<Self::Output> {
        self.guard.before(&mut req).await?;

        // Capture ambient state here — post-guard, while the request scope and
        // the ambient executor/ability are still reachable — and stash it in
        // the request extensions. rmcp forwards those extensions (as
        // `http::request::Parts`) into every operation's `RequestContext`, so
        // `PropagatingHandler` can re-install them *inside* the spawned
        // dispatch, where a task-local from this task would not reach.
        let scope = req.extensions().get::<Arc<RequestScope>>().cloned();
        let captured = self.context.as_ref().map(|context| context.capture(&req));
        // The guard captures for its own `around` the same way — post-`before`,
        // so it sees the ability its chain just attached.
        let guard_captured = self.guard.capture(&req);
        req.extensions_mut().insert(McpAmbient {
            scope: scope.clone(),
            captured,
            guard_captured,
        });

        // Also install the scope here, so an operation rmcp happens to resolve
        // inline (rather than on a spawned task) is covered by the same seam.
        crate::scope::maybe_with_request_scope(scope, self.inner.call(req)).await
    }
}
