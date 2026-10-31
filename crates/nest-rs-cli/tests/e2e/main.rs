//! `nestrs` CLI suite against the dev container's Postgres: what the
//! generators write, run as a developer runs it — the tutorial replayed from
//! its pages, its suite passing on a live database.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

#[path = "../harness/generated.rs"]
mod generated;
mod scaffold;
