//! MCP transport — `#[mcp]` mounts tools on the existing HTTP transport.
//!
//! Unlike HTTP / GraphQL / Queue / Schedule, this crate ships no `McpModule`
//! and no `Transport` impl. MCP is **not a transport**, it is a graft on
//! `HttpTransport` (the same pattern as WS): `#[mcp]` on a struct emits an
//! `endpoint()` factory that mounts under the HTTP server. Apps activate MCP
//! by listing the `#[mcp]`-decorated provider — no `<Transport>Module`
//! activation seam to import.
mod endpoint;
mod guard;
mod guards;
mod scope;

pub use endpoint::{endpoint, endpoint_with_guard};
pub use guards::AllowAllMcpGuard;
pub use guard::{BoxFuture, McpOperationGuard};
/// Per-operation accessor for `#[injectable(scope = request)]` providers inside
/// an MCP tool method — the MCP mirror of `nest_rs_http::Scoped<T>`.
pub use scope::Scoped;

pub use rmcp::handler::server::router::tool::ToolRouter;
pub use rmcp::handler::server::wrapper::Parameters;
pub use rmcp::model::{CallToolResult, Content};
pub use rmcp::{ErrorData as McpError, ServerHandler, schemars, tool, tool_handler, tool_router};

pub use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
pub use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
};

pub use nest_rs_mcp_macros::mcp;
