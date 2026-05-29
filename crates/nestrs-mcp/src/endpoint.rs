//! The poem endpoint that serves an MCP handler over streamable HTTP.

use std::sync::Arc;

use poem::endpoint::TowerCompatExt;
use poem::IntoEndpoint;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::ServerHandler;

/// The factory runs on every new MCP session, so per-session state in the
/// returned handler is fresh.
pub fn endpoint<F, H>(factory: F) -> impl IntoEndpoint
where
    F: Fn() -> H + Send + Sync + 'static,
    H: ServerHandler + Send + 'static,
{
    let service = StreamableHttpService::new(
        move || Ok(factory()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    service.compat()
}
