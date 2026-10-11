//! Integration tests mirroring `src/`: a guard's `check_http` decision renders
//! the right transport response, a chain short-circuits on a denial, and a
//! route's body pipes read through the edge like any reader.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod dispatch;
mod endpoint;
mod guard;
