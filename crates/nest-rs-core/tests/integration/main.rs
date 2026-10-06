//! Integration tests mirroring `src/`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]
mod access;
mod app;
mod container;
mod error_message;
mod lifecycle;
mod module;
mod net;
mod way_down;
