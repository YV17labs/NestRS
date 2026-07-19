//! Integration tests mirroring `src/` (see CLAUDE.md).
//!
//! Documented gaps for the initial pass:
//! - `src/context.rs` — trait-only seam; exercised by the data-context bridge
//!   tests in `nest-rs-seaorm/tests/e2e/ws.rs`.
//! - `src/module.rs` — DI/`#[module]` wiring; exercised by app e2e
//!   (`apps/live/tests/e2e/main.rs`, `apps/api/tests/e2e/main.rs`).
//! - `src/server.rs` — `WsServer` registry has inline `#[cfg(test)] mod tests`.
//! - `src/envelope.rs`, `src/guard.rs` — coverage to add when next touched.

mod diagnostics;
mod gateway;
