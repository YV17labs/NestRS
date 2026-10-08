//! Integration tests mirroring `src/`; the `SocketContext` seam is exercised by
//! `nest-rs-seaorm/tests/e2e/ws.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod gateway;
mod guard_chain;
mod module;
