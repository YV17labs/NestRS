//! The poem endpoint that serves an MCP handler over streamable HTTP.

use std::sync::Arc;

use poem::endpoint::TowerCompatExt;
use poem::{Endpoint, IntoEndpoint, Request, Response, Result, Route};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::ServerHandler;

use crate::guard::McpOperationGuard;

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
    match guard {
        Some(guard) => Route::new().at("/", GuardedEndpoint { guard, inner }),
        None => Route::new().at("/", inner),
    }
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
        self.inner.call(req).await
    }
}
