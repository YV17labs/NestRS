//! Ability-based authorization — transport-agnostic engine plus feature-gated
//! transport bindings.
//!
//! An [`AbilityFactory`] builds an [`Ability`] for the app's actor, which
//! answers three questions backed by one shared [`Predicate`]: `can` (gate an
//! action), `condition_for` (lower rules to a `sea_orm::Condition` for
//! row-level filtering), and `mask` (strip disallowed instances + fields from a
//! response).
//!
//! A rule names a [`Subject`]: an entity, or a capability no table holds that
//! [`#[subject]`](subject) declares, which a rule grants whole and a posture
//! gates without masking.
//!
//! [`AbilityGuard`] is the one guard, answering all four edges.
//!
//! Bindings: `http`, `graphql`, `ws`, `mcp`. The data-coupled binding — the
//! row `#[authorize(A, bind = S)]` loads — lives in `nest-rs-seaorm`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod ability;
mod action;
mod builder;
mod context;
mod error;
mod factory;
mod gate;
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
mod guard;
mod mask;
mod predicate;
mod subject;
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
mod wire_mask;
mod wire_model;

/// This crate's span target.
pub const TARGET: &str = "nest_rs::authz";

pub use ability::{Ability, FieldSet};
pub use action::{Action, ActionMarker, Create, Delete, Manage, Read, Update};
pub use builder::{AbilityBuilder, RuleSpec};
pub use context::{current_ability, with_ability};
pub use error::{MalformedRuleError, MaskReplyError};
pub use factory::AbilityFactory;
pub use gate::class_gate;
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
pub use guard::AbilityGuard;
pub use mask::masked_output_ambient;
pub use predicate::{Predicate, PredicateBuilder};
pub use subject::Subject;

/// Declares a unit struct a [`Subject`] no table holds — a capability a rule
/// grants whole and `#[authorize(Action, Subject)]` gates on every edge,
/// arming no mask since the subject has no rows.
///
/// ```
/// use nest_rs_authz::{AbilityBuilder, Action, subject};
///
/// /// May export the org's reports.
/// #[subject]
/// pub struct Exporting;
///
/// let mut ab = AbilityBuilder::new();
/// ab.can(Action::Manage, Exporting).requires_scope("reports:export");
/// # ab.build()?;
/// # Ok::<(), nest_rs_authz::MalformedRuleError>(())
/// ```
///
/// A rule on it takes no `when` and no `fields`, which narrow an entity's rows
/// and columns, and no `cannot`: the class gate is the subject's one layer, so
/// a grant not written is the denial.
///
/// # Expands to
///
/// The struct unchanged, plus its `Subject` impl and the hidden impl that tells
/// every edge's posture it has no rows to mask.
pub use nest_rs_authz_macros::subject;
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
pub use wire_mask::masked_reply;
pub use wire_model::WireModelDefaults;

#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "mcp")]
pub mod mcp;
#[cfg(feature = "ws")]
pub mod ws;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    #[cfg(feature = "graphql")]
    pub use crate::graphql::mask::{GraphqlMask, graphql_masked_item, graphql_masked_value};
    #[cfg(feature = "http")]
    pub use crate::http::mask::RouteMask;
    #[cfg(feature = "mcp")]
    pub use crate::mcp::mask::{McpMask, mcp_masked_value};
    pub use crate::subject::{NoRows, Rows, SubjectMask};
    #[cfg(feature = "ws")]
    pub use crate::ws::mask::{WsMask, ws_masked_reply};
}
