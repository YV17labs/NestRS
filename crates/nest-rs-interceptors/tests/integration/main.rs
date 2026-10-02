//! Integration coverage for `nest-rs-interceptors` — the crate's public API in
//! process, no DB/network. Composition *order* is a wiring property that unit
//! tests on a single interceptor can't show (HTTP-T2).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod ordering;
