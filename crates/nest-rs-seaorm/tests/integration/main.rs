//! `nest-rs-seaorm`'s in-process suite: what the exposure decorators emit, in
//! `expose/`, which mirrors their macro crate's `src/` — their refusals are
//! trybuild snapshots in `nest-rs-macro-hygiene`'s suite — the pool's refusal
//! of a server it cannot verify, before any login, the indicator the pool's
//! module attaches, in `health/`, the unit of work under the
//! HTTP edge's deadline, a page cursor refused at every edge reading one, and
//! what a `#[resource]`'s binding answers over a service standing in for the row.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

#[path = "../harness/always_authenticated.rs"]
mod always_authenticated;
mod bind;
mod database;
mod error;
mod expose;
mod graphql;
mod health;
mod http;
mod module;
mod page;
mod repo;
mod savepoint;
mod service;
mod unit;

/// The principal the suite's authentication stand-ins attach: a caller with no
/// identity of its own.
#[derive(Clone, Debug, Default)]
pub struct Anyone;

impl nest_rs_core::AmbientValue for Anyone {}

/// A test app over a pool and a config seeded unconnected, for a module that
/// imports `SeaOrmDatabaseModule` and refuses before any statement runs.
pub(crate) fn over_an_unconnected_pool() -> nest_rs_testing::TestAppBuilder {
    nest_rs_testing::TestApp::builder()
        .provide(sea_orm::DatabaseConnection::default())
        .provide(nest_rs_seaorm::SeaOrmConfig::default())
}

/// A test app over a pool whose every acquire is refused: nothing listens on port 1.
pub(crate) async fn over_a_pool_refusing_every_acquire() -> nest_rs_testing::TestAppBuilder {
    let mut options = sea_orm::ConnectOptions::new("postgres://nestrs@127.0.0.1:1/nowhere");
    options
        .connect_lazy(true)
        .acquire_timeout(std::time::Duration::from_millis(50));
    let db = sea_orm::Database::connect(options)
        .await
        .expect("a lazy pool opens nothing");
    nest_rs_testing::TestApp::builder().provide(db)
}
