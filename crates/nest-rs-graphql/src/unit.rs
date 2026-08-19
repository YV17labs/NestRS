//! The canonical names of the units of work this edge opens.
//!
//! Declared here rather than in the kernel, and held to the `<edge>.<unit>`
//! grammar by the `units` join in `nest-rs-conformance`: both are argued once,
//! in [`nest_rs_core::operation_log`].
//!
//! **Two units, because this edge dispatches at two granularities and only one
//! of them is visible from both.** A query, a mutation, an entity reference and
//! a field resolver are each dispatched by this crate's own `#[operations]`
//! expansion, which knows the field being resolved — so the field is the unit
//! and [`OPERATION`] names it. A subscription is served by async-graphql's own
//! message loop, which this crate never sees an operation boundary inside, so
//! the connection is the unit and [`SUBSCRIPTION`] names that.
//!
//! The asymmetry is the standard's, not a choice: what a site cannot see it
//! cannot name, and levelling the two down to one connection-shaped unit would
//! make "which query was slow?" unanswerable for the majority of GraphQL
//! traffic — which is what it was, since a query's only line was the
//! `POST /graphql` the HTTP edge filed for the whole document.

/// One dispatched GraphQL field — a `#[query]`, `#[mutation]`, `#[entity]` or
/// `#[field_resolver]`.
pub const OPERATION: &str = "graphql.operation";

/// One GraphQL subscription; the connection is the unit of work.
pub const SUBSCRIPTION: &str = "graphql.subscription";
