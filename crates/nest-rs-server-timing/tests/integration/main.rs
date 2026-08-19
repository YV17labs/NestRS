//! Integration tests mirroring `src/` (see CLAUDE.md).
//!
//! Documented gaps (no test file required): `src/lib.rs` re-exports only;
//! `src/module.rs` is a bare `#[module]`, exercised by the boot in
//! `interceptor`; `src/entry.rs` and `src/format.rs` carry their own
//! `#[cfg(test)]` units.

mod interceptor;
