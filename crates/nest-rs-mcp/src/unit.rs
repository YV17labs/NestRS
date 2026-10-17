//! The canonical name of the unit of work this edge opens.

use nest_rs_core::operation_log::Unit;

/// One MCP operation — a request or a notification.
pub const OPERATION: Unit =
    nest_rs_core::unit!("mcp.operation", target: crate::TARGET, kind: Server);

#[cfg(test)]
mod tests {
    use nest_rs_core::Edge;

    use super::*;

    #[test]
    fn the_unit_names_its_edge() {
        assert_eq!(OPERATION.edge(), Some(Edge::Mcp));
    }
}
