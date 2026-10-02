//! Integration tests mirroring `src/` (see CLAUDE.md). `nest-rs-guards` owns
//! the auth chain, so its guard→response wiring is exercised here in-process
//! (no DB/network): a guard's `check_http` decision must render the right
//! transport response, and a chain must run each guard and short-circuit on a
//! denial.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod endpoint;
