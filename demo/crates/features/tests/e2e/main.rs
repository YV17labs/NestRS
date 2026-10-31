#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]
mod audio;

mod notifications;

mod orgs;

mod posts;

mod users;

use std::sync::Arc;

use nest_rs::core::module;
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs::testing::TestApp;
use sea_orm::DatabaseConnection;

#[module(imports = [SeaOrmModule, SeaOrmDatabaseModule])]
struct DataModule;

pub(crate) async fn data_app() -> TestApp {
    TestApp::builder()
        .module::<DataModule>()
        .ephemeral_database::<migrations::Migrator>()
        .build()
        .await
        .expect("the data layer boots on a throwaway database")
}

pub(crate) fn connection(app: &TestApp) -> Arc<DatabaseConnection> {
    app.container()
        .get::<DatabaseConnection>()
        .expect("the app's pool is its throwaway database")
}
