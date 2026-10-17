//! Integration suite for `nest-rs-database`: the unit-of-work port through
//! `run_unit`, against bindings written here the way a driver writes one.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod unit;
