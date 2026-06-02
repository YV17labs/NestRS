//! MCP transport — `#[mcp]` mounts tools on the existing HTTP transport.
//!
//! Integration tests: exercised via `apps/mcp` e2e; crate-local guard seam in
//! `tests/guard.rs`.

mod endpoint;
mod guard;

pub use endpoint::{endpoint, endpoint_with_guard};
pub use guard::{BoxFuture, McpOperationGuard};

pub use rmcp::handler::server::router::tool::ToolRouter;
pub use rmcp::handler::server::wrapper::Parameters;
pub use rmcp::model::{CallToolResult, Content};
pub use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};

pub use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
pub use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
};

/// The `#[mcp]` decorator, defined in `nestrs-mcp-macros` and surfaced here so
/// apps write `nestrs_mcp::mcp`.
pub use nestrs_mcp_macros::mcp;
