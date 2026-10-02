//! Integration tests mirroring `src/` (see CLAUDE.md).
//!
//! Documented gaps (no test file required): `src/lib.rs` re-exports only;
//! `src/module.rs` is a bare `#[module]`, exercised by the boot in
//! `interceptor`; `src/entry.rs` and `src/format.rs` carry their own
//! `#[cfg(test)]` units.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod interceptor;
