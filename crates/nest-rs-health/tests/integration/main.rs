//! Integration tests mirroring `src/` (see CLAUDE.md).
//!
//! Documented gaps (no test file required): `src/lib.rs` re-exports only;
//! `src/config.rs` is asserted by its own in-file `#[cfg(test)] mod tests`
//! (the norm's item 4) and through the boot in `module` below.
//!
//! `indicator` covers `src/indicator.rs` — not the data, which `service`
//! exercises through hand-built entries, but the `#[indicators]` expansion
//! that fills it, which neither umbrella witness executes.
//!
//! `controller` is this capability's **composition witness** — the documented
//! import, booted, answering. It used to be listed among the gaps above, on the
//! grounds that "every app importing `HealthModule`" exercises it end-to-end;
//! those apps are in `demo/`, so the capability's own crate proved none of it,
//! which is the obligation `CLAUDE.md`'s *Shipping a new capability* puts on the
//! capability rather than on its consumers.
mod controller;
mod diagnostics;
mod indicator;
mod module;
mod service;
