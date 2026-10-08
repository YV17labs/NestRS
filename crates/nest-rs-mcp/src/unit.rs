//! The canonical name of the unit of work this edge opens.

use nest_rs_core::operation_log::Unit;

/// One MCP operation — a request or a notification.
pub const OPERATION: Unit =
    nest_rs_core::unit!("mcp.operation", target: crate::TARGET, kind: Server);
