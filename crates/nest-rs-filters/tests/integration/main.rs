//! Integration coverage for `nest-rs-filters` — the crate's public API in
//! process, no DB/network. Which filter maps an error when several are stacked
//! is a wiring property a single-filter unit test can't show (HTTP-T3).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod ordering;
