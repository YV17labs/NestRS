//! CASL-style authorization for nestrs — the transport-agnostic engine, plus
//! feature-gated transport bindings.
//!
//! Rules are declared once by an [`AbilityFactory`] for the app's actor type,
//! producing an [`Ability`] that answers three questions, all backed by one
//! shared [`Predicate`] representation so they cannot drift apart:
//!
//! 1. **Can the actor?** — [`Ability::can_class`] (class-level) and
//!    [`Ability::can`] (instance-level) gate an action on a subject.
//! 2. **Which rows?** — [`Ability::condition_for`] lowers the matching rules'
//!    conditions to a [`sea_orm::Condition`] the data layer applies, so a query
//!    returns only the rows the actor may see.
//! 3. **Which fields?** — [`Ability::mask`] / [`Ability::mask_many`] drop
//!    disallowed instances and strip disallowed fields from a response body.
//!
//! Transport bindings live in feature-gated submodules: [`http`] (the
//! `Authorize` extractor, `AbilityGuard`, `Scope`, and the response shaper),
//! [`graphql`] (the per-operation bridge, the `authorize` gate, the context
//! seed), [`mcp`] (the MCP operation guard). An app pulls only the ones it
//! serves: `nestrs-authz = { workspace = true, features = ["http", "graphql"] }`.
//! The data-coupled bindings (`Bind`, the GraphQL `bind` helper, `LoaderScope`,
//! `WsDataContext`) live in `nestrs-database` behind matching features so the
//! engine stays free of a data-layer dependency.

mod ability;
mod action;
mod builder;
mod context;
mod factory;
mod predicate;
mod subject;

pub use ability::{Ability, FieldSet};
pub use action::{Action, ActionMarker, Create, Delete, Manage, Read, Update};
pub use builder::{AbilityBuilder, RuleSpec};
pub use context::{current_ability, with_ability};
pub use factory::AbilityFactory;
pub use predicate::{Predicate, PredicateBuilder};
pub use subject::Subject;

#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "mcp")]
pub mod mcp;
