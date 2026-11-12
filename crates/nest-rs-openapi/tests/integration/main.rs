//! Public-API exercise for `nest-rs-openapi`: what a caller gets back from
//! `/api-json` and `/api`, enabled or not.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod document;
mod module;
