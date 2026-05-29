//! CASL-style authorization for nestrs — the transport-agnostic engine.
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
//! The HTTP surface that drives these per request lives in `nestrs-authz-http`.

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
