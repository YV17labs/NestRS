//! Integration tests mirroring `src/` (see CLAUDE.md).
//!
//! Documented gaps (no test file required): `src/lib.rs` re-exports only;
//! `src/controller.rs` is exercised end-to-end by every app importing
//! `HealthModule` (see `apps/api/tests/e2e.rs::health_probe_*`);
//! `src/indicator.rs` is data + an `inventory::collect!` site, exercised
//! through `service` below.
mod module;
mod service;
