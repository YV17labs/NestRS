//! Integration tests mirroring `src/` (see CLAUDE.md).
//!
//! Documented gaps (no test file required): `src/lib.rs` re-exports only;
//! `src/indicator.rs` is data + an `inventory::collect!` site, exercised
//! through `service` below.
//!
//! `controller` is this capability's **composition witness** — the documented
//! import, booted, answering. It used to be listed among the gaps above, on the
//! grounds that "every app importing `HealthModule`" exercises it end-to-end;
//! those apps are in `demo/`, so the capability's own crate proved none of it,
//! which is the obligation `CLAUDE.md`'s *Shipping a new capability* puts on the
//! capability rather than on its consumers.
mod controller;
mod diagnostics;
mod module;
mod service;
