//! MCP bridge (feature `mcp`): re-installs the request's ambient
//! [`Executor`](crate::Executor) and ability inside each tool dispatch, across
//! rmcp's spawn.

mod context;

pub use context::McpDataContext;
