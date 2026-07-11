//! The poem endpoint that serves an MCP handler over streamable HTTP.

use std::sync::Arc;

use nest_rs_core::RequestScope;
use poem::endpoint::TowerCompatExt;
use poem::{Endpoint, IntoEndpoint, Request, Response, Result, Route};
use rmcp::ServerHandler;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use crate::guard::McpOperationGuard;
use crate::guards::DenyAllMcpGuard;

/// `factory` runs on every new MCP session, so per-session state stays fresh.
pub fn endpoint<F, H>(factory: F) -> impl IntoEndpoint
where
    F: Fn() -> H + Send + Sync + 'static,
    H: ServerHandler + Send + 'static,
{
    endpoint_with_guard(None, factory)
}

pub fn endpoint_with_guard<F, H>(
    guard: Option<Arc<dyn McpOperationGuard>>,
    factory: F,
) -> impl IntoEndpoint
where
    F: Fn() -> H + Send + Sync + 'static,
    H: ServerHandler + Send + 'static,
{
    let service = StreamableHttpService::new(
        move || Ok(factory()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let inner = service.compat();
    // Fail closed: an MCP endpoint mounted without an explicit
    // `McpOperationGuard` denies every request rather than serving the tool
    // surface unauthenticated. Both `endpoint` and the `#[mcp]` macro funnel
    // through here, so a missing guard can never silently open `/mcp`.
    let guard = guard.unwrap_or_else(|| {
        // Say so once at boot — mirrors GraphQL's unguarded-schema warning so a
        // deny-all endpoint born of a missing guard import is never silent.
        tracing::warn!(
            target: "nest_rs::mcp",
            mode = "deny_all",
            "no operation guard registered — mcp endpoint is deny-all",
        );
        Arc::new(DenyAllMcpGuard)
    });
    Route::new().at("/", GuardedEndpoint { guard, inner })
}

struct GuardedEndpoint<E> {
    guard: Arc<dyn McpOperationGuard>,
    inner: E,
}

impl<E> Endpoint for GuardedEndpoint<E>
where
    E: Endpoint<Output = Response>,
{
    type Output = Response;

    async fn call(&self, mut req: Request) -> Result<Self::Output> {
        self.guard.before(&mut req).await?;
        // Install the per-operation request scope (forwarded from the HTTP
        // extensions by `RequestScopeEndpoint`, which wraps the whole route
        // tree) as a task-local for the duration of the call, so tool methods
        // reach request-scoped providers via `nest_rs_mcp::Scoped<T>`. Absent
        // (no `RequestScopeEndpoint` in front) ⇒ run without a scope; a tool's
        // `Scoped::from_context` then surfaces the wiring error.
        match req.extensions().get::<Arc<RequestScope>>().cloned() {
            Some(scope) => crate::scope::with_request_scope(scope, self.inner.call(req)).await,
            None => self.inner.call(req).await,
        }
    }
}
