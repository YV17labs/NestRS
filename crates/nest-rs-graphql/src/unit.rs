//! The canonical name of the unit of work this edge opens.
//!
//! Declared here rather than in the kernel, and held to the `<edge>.<unit>`
//! grammar by the `units` join in `nest-rs-conformance`: both are argued once,
//! in [`nest_rs_core::operation_log`].
//!
//! A query or mutation is served inside one HTTP request, which is `http.request`'s
//! unit; a subscription outlives it, and nothing below the connection is a unit
//! this crate can see, so the connection is the unit of work.

/// One GraphQL subscription; the connection is the unit of work.
pub const SUBSCRIPTION: &str = "graphql.subscription";
