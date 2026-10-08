//! Integration tests for the config crate's public macro surface.
//!
//! Needs nextest: each test freezes process-global state (the prefix, the
//! claim registry) and must own its process.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod dotenv;
mod env_prefix;
mod namespace;
mod service;
mod unclaimed;
