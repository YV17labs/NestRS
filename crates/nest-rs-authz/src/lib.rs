//! CASL-style authorization — transport-agnostic engine plus feature-gated
//! transport bindings.
//!
//! An [`AbilityFactory`] builds an [`Ability`] for the app's actor, which
//! answers three questions backed by one shared [`Predicate`] (so they can't
//! drift apart): `can` (gate an action), `condition_for` (lower rules to a
//! `sea_orm::Condition` for row-level filtering), and `mask` (strip
//! disallowed instances + fields from a response).
//!
//! Bindings: `http`, `graphql`, `mcp`. The data-coupled bindings
//! (`Bind`, the GraphQL `bind` helper, `LoaderScope`, `WsDataContext`) live in
//! `nest-rs-seaorm` so the engine stays free of a data-layer dependency.
#![warn(missing_docs)]

mod ability;
mod action;
mod builder;
// The shared per-operation guard chain: only the two `Exempt`-edge transports
// run it themselves (HTTP gates in the route shaper's pool instead).
#[cfg(any(feature = "graphql", feature = "mcp"))]
mod chain;
mod context;
mod factory;
mod mask;
mod predicate;
mod subject;
#[cfg(any(feature = "http", feature = "graphql"))]
mod wire_mask;

pub use ability::{Ability, FieldSet, MalformedRuleError};
pub use action::{Action, ActionMarker, Create, Delete, Manage, Read, Update};
pub use builder::{AbilityBuilder, RuleSpec};
#[cfg(any(feature = "graphql", feature = "mcp"))]
pub use chain::run_ability_chain;
pub use context::{current_ability, with_ability};
pub use factory::AbilityFactory;
pub use mask::masked_output_ambient;
pub use predicate::{Predicate, PredicateBuilder};
pub use subject::Subject;
#[cfg(any(feature = "http", feature = "graphql"))]
pub use wire_mask::{MaskReplyError, masked_reply};

#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "mcp")]
pub mod mcp;
