//! Integration tests for the config crate's public macro surface.
//!
//! The custom-prefix tests set `NESTRS_ENV_PREFIX` and read it back in the same
//! process, which nextest makes honest: it runs every test in its own process,
//! so the `OnceLock` each one freezes is its own. Bare `cargo test` would share
//! one process between them and is unsupported for exactly this class of reason.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod diagnostics;
mod dotenv;
mod env_prefix;
mod namespace;
mod service;
mod unclaimed;
