//! Suite root: one module per structural check.
//!
//! Each module reads file paths, manifests or declared constants and fails on
//! a layout the rules forbid. None of them proves a rule about what code
//! *does* — that is held by types, by clippy and by behaviour tests, a rung up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod filters;
mod keys;
mod naming;
mod paths;
mod snapshots;
mod targets;
mod umbrella;
