//! Ability-based authorization — transport-agnostic engine plus feature-gated
//! transport bindings.
//!
//! An [`AbilityFactory`] builds an [`Ability`] for the app's actor, which
//! answers three questions backed by one shared [`Predicate`]: `can` (gate an
//! action), `condition_for` (lower rules to a `sea_orm::Condition` for
//! row-level filtering), and which rows and columns of an entity a caller may
//! read, which `#[expose]`'s conversion asks where it builds a wire row.
//!
//! A rule names a [`Subject`]: an entity, or a capability no table holds that
//! [`#[subject]`](macro@subject) declares, which a rule grants whole and a posture
//! gates. Either posture opens its unit's reads to the caller's ability.
//!
//! `AbilityGuard` is the one guard, answering all four edges; each binding's
//! feature turns it on.
//!
//! Bindings: `http`, `graphql`, `ws`, `mcp`. The data-coupled binding — the
//! row `#[authorize(A, bind = S)]` loads — lives in `nest-rs-seaorm`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod ability;
mod action;
mod builder;
mod contained;
mod context;
mod error;
mod factory;
mod gate;
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
mod guard;
mod judged;
mod predicate;
mod subject;

/// This crate's span target.
pub const TARGET: &str = "nest_rs::authz";

pub use ability::{Ability, FieldSet};
pub use action::{Action, ActionMarker, Create, Delete, Manage, Read, Update};
pub use builder::{AbilityBuilder, Can, Cannot, RuleSpec};
pub use context::{current_ability, with_ability};
pub use error::{ColumnsRefused, MalformedRuleError};
pub use factory::AbilityFactory;
pub use gate::class_gate;
#[cfg(any(feature = "http", feature = "graphql", feature = "ws", feature = "mcp"))]
pub use guard::AbilityGuard;
pub use predicate::{Predicate, PredicateBuilder};
pub use subject::{RowSubject, Subject};

/// Declares a unit struct a [`Subject`] no table holds — a capability a rule
/// grants whole and `#[authorize(Action, Subject)]` gates on every edge, opening
/// no read since the subject has no rows.
///
/// ```
/// use nest_rs_authz::{AbilityBuilder, Action, subject};
/// use nest_rs_guards::Scope;
///
/// /// May export the org's reports.
/// #[subject]
/// pub struct Exporting;
///
/// const REPORTS_EXPORT: Scope = Scope::new("reports:export");
///
/// let mut ab = AbilityBuilder::new();
/// ab.can(Action::Manage, Exporting).requires_scope(REPORTS_EXPORT);
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
/// The struct unchanged, plus its `Subject` impl, which tells every edge's posture
/// it has no rows to read.
pub use nest_rs_authz_macros::subject;

#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "mcp")]
mod mcp;
#[cfg(feature = "ws")]
mod ws;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::contained::{Uncontained, contained_text, contained_value};
    pub use crate::context::{AmbientAbility, install_ability, visitors_read_unnamed};
    #[cfg(feature = "graphql")]
    pub use crate::judged::judged_field;
    pub use crate::judged::{
        Judged, JudgedRow, Judgement, PushedItem, PushedItemFallback, PushedRows, Refused,
        WireColumns, judge,
    };
    pub use inventory;
    pub use nest_rs_core::__private::{RefusedAnswer, WireRow};
}
