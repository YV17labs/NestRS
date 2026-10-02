//! Integration tests for `nest-rs-oauth-server`. Layout mirrors `src/`.
//!
//! `registry.rs` is covered by the unit tests beside it: constant-time
//! comparison and the miss paths are properties of the function, not of a
//! mounted endpoint, so a boot would add nothing an in-file test cannot see.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod error;
mod token;
