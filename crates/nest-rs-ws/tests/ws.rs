//! Integration tests mirroring `src/` (see CLAUDE.md).
//!
//! Documented gaps for the initial pass:
//! - `src/context.rs` — trait-only seam; exercised by the data-context bridge
//!   tests in `nestrs-seaorm/tests/ws.rs`.
//! - `src/module.rs` — DI/`#[module]` wiring; exercised by app e2e
//!   (`apps/live/tests/e2e.rs`, `apps/api/tests/e2e.rs`).
//! - `src/server.rs` — `WsServer` registry has inline `#[cfg(test)] mod tests`.
//! - `src/envelope.rs`, `src/guard.rs` — coverage to add when next touched.

mod gateway;
