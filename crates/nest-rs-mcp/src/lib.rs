//! MCP transport — `#[mcp]` mounts tools on the existing HTTP transport.

mod endpoint;
mod guard;

pub use endpoint::{endpoint, endpoint_with_guard};
pub use guard::{BoxFuture, McpOperationGuard};

pub use rmcp::handler::server::router::tool::ToolRouter;
pub use rmcp::handler::server::wrapper::Parameters;
pub use rmcp::model::{CallToolResult, Content};
pub use rmcp::{ErrorData as McpError, ServerHandler, schemars, tool, tool_handler, tool_router};

pub use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
pub use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
};

pub use nest_rs_mcp_macros::mcp;
