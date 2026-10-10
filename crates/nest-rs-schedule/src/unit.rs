//! The canonical name of the unit of work this edge opens.

use nest_rs_core::operation_log::Unit;

/// One scheduled tick.
pub const TICK: Unit = nest_rs_core::unit!("schedule.tick", target: crate::TARGET, kind: Internal);

#[cfg(test)]
mod tests {
    use nest_rs_core::Edge;

    use super::*;

    #[test]
    fn the_unit_names_its_edge() {
        assert_eq!(TICK.edge(), Some(Edge::Schedule));
    }
}
