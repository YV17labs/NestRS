use std::sync::Arc;

use poem::endpoint::TowerCompatExt;
use poem::IntoEndpoint;

pub use rmcp::handler::server::router::tool::ToolRouter;
pub use rmcp::handler::server::wrapper::Parameters;
pub use rmcp::model::{CallToolResult, Content};
pub use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};

pub use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
pub use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
};

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
