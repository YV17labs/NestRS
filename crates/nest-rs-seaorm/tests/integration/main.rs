//! In-process suite for `nest-rs-seaorm`'s compile-time contracts. The
//! Postgres-backed behaviour lives next door in `tests/e2e/`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod diagnostics;
mod soft_delete;
