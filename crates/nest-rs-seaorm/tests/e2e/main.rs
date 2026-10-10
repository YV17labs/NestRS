//! `nest-rs-seaorm`'s suite against the dev container's Postgres: every test
//! here needs the database. What the decorators emit runs in process, in
//! `tests/integration`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod harness;

mod create;
mod database;
#[cfg(feature = "graphql")]
mod graphql;
mod interceptor;
mod lazy;
mod lifecycle_hooks;
mod module;
mod public_visitor;
mod relational_authz;
mod scope;
mod worker;
#[cfg(feature = "ws")]
mod ws;
