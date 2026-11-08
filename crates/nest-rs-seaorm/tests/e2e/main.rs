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

#[cfg(feature = "graphql")]
#[path = "../harness/always_authenticated.rs"]
mod always_authenticated;
mod atomic;
mod create;
mod database;
mod error;
mod events;
mod executor;
#[cfg(feature = "graphql")]
mod graphql;
mod health;
mod http;
mod lazy;
mod lifecycle_hooks;
mod mcp;
mod module;
mod mutual_tls;
mod public_visitor;
mod queue;
mod relational_authz;
mod repo;
mod scope;
mod select;
mod service;
mod soft_delete;
#[path = "../harness/system_store.rs"]
mod system_store;
mod unique_key;
mod unit;
mod worker;
mod ws;

/// The principal the suite's authentication stand-ins attach: a caller with no
/// identity of its own.
#[derive(Clone, Debug, Default)]
pub struct Anyone;

impl nest_rs_core::AmbientValue for Anyone {}

impl nest_rs_authn::PrincipalIdentity for Anyone {
    fn actor_id(&self) -> Option<String> {
        None
    }
}
