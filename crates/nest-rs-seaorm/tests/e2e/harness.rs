//! Shared Postgres connection for the e2e suite — one place for the env-var
//! contract instead of a copy per module.

use std::sync::Arc;

use sea_orm::{Database, DatabaseConnection};

pub(crate) async fn connect() -> DatabaseConnection {
    let url = std::env::var("NESTRS_DATABASE__URL")
        .expect("NESTRS_DATABASE__URL must point at a reachable Postgres for this test");
    Database::connect(&url).await.expect("connect to Postgres")
}

pub(crate) async fn connect_arc() -> Arc<DatabaseConnection> {
    Arc::new(connect().await)
}
