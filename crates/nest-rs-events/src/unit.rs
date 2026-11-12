//! The canonical name of the unit of work this edge opens: one listener
//! invocation, not one `emit`, which runs inside the emitter's own unit.

use nest_rs_core::operation_log::Unit;

/// One listener invocation for one emitted event.
pub const DISPATCH: Unit =
    nest_rs_core::unit!("events.dispatch", target: crate::TARGET, kind: Internal);

#[cfg(test)]
mod tests {
    use nest_rs_core::Edge;

    use super::*;

    #[test]
    fn the_unit_names_its_edge() {
        assert_eq!(DISPATCH.edge(), Some(Edge::Events));
    }
}
