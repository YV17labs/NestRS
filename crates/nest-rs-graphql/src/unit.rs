//! The canonical names of the units of work this edge opens, declared through
//! [`nest_rs_core::unit!`].
//!
//! Each name is the GraphQL term for what one dispatch serves: the document's
//! operation ([`OPERATION`]), a root field `#[operations]` dispatches itself
//! ([`FIELD`]), and the connection a subscription runs on
//! ([`SUBSCRIPTION`]), since its stream runs inside async-graphql's protocol
//! engine, which this crate never sees run. A field resolved below a root
//! field is no unit: its root field and the operation report it.

use nest_rs_core::operation_log::Unit;

/// One operation of a GraphQL document — its `query`, `mutation` or
/// `subscription`.
pub const OPERATION: Unit =
    nest_rs_core::unit!("graphql.operation", target: crate::TARGET, kind: Server);

/// One dispatched root field — a `#[query]`, `#[mutation]` or `#[entity]`.
pub const FIELD: Unit = nest_rs_core::unit!("graphql.field", target: crate::TARGET, kind: Server);

/// One GraphQL subscription; the connection is the unit of work.
pub const SUBSCRIPTION: Unit =
    nest_rs_core::unit!("graphql.subscription", target: crate::TARGET, kind: Server);

#[cfg(test)]
mod tests {
    use nest_rs_core::Edge;

    use super::*;

    #[test]
    fn every_unit_names_its_edge() {
        for unit in [OPERATION, FIELD, SUBSCRIPTION] {
            assert_eq!(unit.edge(), Some(Edge::Graphql), "{}", unit.name());
        }
    }
}
