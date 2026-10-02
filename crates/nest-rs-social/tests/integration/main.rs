//! Integration coverage for the social provider seam: link-time entry
//! submission and the flow-owning trait's default delegation.
//!
//! Reachability filtering and the fail-boot validation rules are unit-tested
//! inside `src/registry.rs` (they need the crate-private `install`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod provider;
mod providers;
