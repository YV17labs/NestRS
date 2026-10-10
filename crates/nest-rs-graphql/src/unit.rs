//! The canonical names of the units of work this edge opens, declared through
//! [`nest_rs_core::unit!`].
//!
//! `#[operations]` dispatches each field itself, so the field is the unit
//! ([`OPERATION`]); a subscription runs inside async-graphql's protocol engine,
//! which this crate never sees run, so the connection is ([`SUBSCRIPTION`]).

use nest_rs_core::operation_log::Unit;

/// One dispatched GraphQL field — a `#[query]`, `#[mutation]`, `#[entity]` or
/// `#[field_resolver]`.
pub const OPERATION: Unit =
    nest_rs_core::unit!("graphql.operation", target: crate::TARGET, kind: Server);

/// One GraphQL subscription; the connection is the unit of work.
pub const SUBSCRIPTION: Unit =
    nest_rs_core::unit!("graphql.subscription", target: crate::TARGET, kind: Server);

#[cfg(test)]
mod tests {
    use nest_rs_core::Edge;

    use super::*;

    #[test]
    fn every_unit_names_its_edge() {
        for unit in [OPERATION, SUBSCRIPTION] {
            assert_eq!(unit.edge(), Some(Edge::Graphql), "{}", unit.name());
        }
    }
}
