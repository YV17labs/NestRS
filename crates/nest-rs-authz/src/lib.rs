//! Ability-based authorization — transport-agnostic engine plus feature-gated
//! transport bindings.
//!
//! An [`AbilityFactory`] builds an [`Ability`] for the app's actor, which
//! answers three questions backed by one shared [`Predicate`]: `can` (gate an
//! action), `condition_for` (lower rules to a `sea_orm::Condition` for
//! row-level filtering), and `mask` (strip disallowed instances + fields from a
//! response).
//!
//! [`AbilityGuard`] is the one guard, answering all four edges.
//!
//! Bindings: `http`, `graphql`, `ws`, `mcp`. The data-coupled bindings
//! (`Bind`, the GraphQL `bind` helper, `LoaderScope`, `WsDataContext`) live in
//! `nest-rs-seaorm`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod ability;
mod action;
mod builder;
// Only the `Exempt`-edge transports run the chain themselves: HTTP gates in the
// route shaper's pool, and a WS gateway's upgrade already ran it.
#[cfg(any(feature = "graphql", feature = "mcp"))]
mod chain;
mod context;
mod error;
mod factory;
// `http` included: `Authorize`'s extractor logs its denial through `gate::warn_denied`.
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
mod gate;
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
mod guard;
mod mask;
mod predicate;
mod subject;
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
mod wire_mask;

/// This crate's span target.
pub const TARGET: &str = "nest_rs::authz";

pub use ability::{Ability, FieldSet};
pub use action::{Action, ActionMarker, Create, Delete, Manage, Read, Update};
pub use builder::{AbilityBuilder, RuleSpec};
#[cfg(any(feature = "graphql", feature = "mcp"))]
pub use chain::run_ability_chain;
pub use context::{current_ability, with_ability};
pub use error::{MalformedRuleError, MaskReplyError};
pub use factory::AbilityFactory;
#[cfg(any(feature = "graphql", feature = "ws", feature = "mcp"))]
pub use gate::{GateVerdict, gate};
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
pub use guard::AbilityGuard;
pub use mask::masked_output_ambient;
pub use predicate::{Predicate, PredicateBuilder};
pub use subject::Subject;
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
pub use wire_mask::masked_reply;

#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "mcp")]
pub mod mcp;
#[cfg(feature = "ws")]
pub mod ws;
