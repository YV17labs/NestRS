//! The canonical name of the unit of work this edge opens.
//!
//! Declared here rather than in the kernel, and held to the `<edge>.<unit>`
//! grammar by the `units` join in `nest-rs-conformance`: both are argued once,
//! in [`nest_rs_core::operation_log`].

/// One MCP operation — a request or a notification.
pub const OPERATION: &str = "mcp.operation";
