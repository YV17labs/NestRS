//! MCP bridge (feature `mcp`) — captures the request's ambient
//! [`Executor`](crate::Executor) + ability and re-installs them inside each
//! tool dispatch, across rmcp's spawn.

mod context;

pub use context::McpDataContext;
