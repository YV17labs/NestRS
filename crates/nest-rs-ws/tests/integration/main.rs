//! Integration tests mirroring `src/`.
//!
//! Documented gaps for the initial pass:
//! - `src/context.rs` — trait-only seam; exercised by the data-context bridge
//!   tests in `nest-rs-seaorm/tests/integration/ws.rs`.
//! - `src/server.rs` — `WsServer` registry has inline `#[cfg(test)] mod tests`.
//! - `src/envelope.rs`, `src/guard.rs` — coverage to add when next touched.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod diagnostics;
mod gateway;
mod guard_chain;
mod module;
