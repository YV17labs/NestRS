//! [`DatabaseModule`] — the async-owned SeaORM connection (see the [crate docs](crate)).
//!
//! Configured at its import site with **`DatabaseModule::for_root()`** (no bare
//! form — a configurable module always carries a visible `for_*`): it routes the
//! load through [`ConfigModule::for_feature`] (no env read of its own), validates,
//! connects, and installs the request layers.

use std::sync::Arc;

use nestrs_config::ConfigModule;
use nestrs_core::{ContainerBuilder, DynamicModule};
use sea_orm::{Database, DatabaseConnection};

use crate::config::DatabaseConfig;

/// The database module. Wire it with `DatabaseModule::for_root()` (env-driven).
/// Registers a `sea_orm::DatabaseConnection` and installs the
/// [`DbContext`](crate::DbContext) request interceptor.
pub struct DatabaseModule;

impl DatabaseModule {
    /// Env-driven: load [`DatabaseConfig`] from `NESTRS_DATABASE__*` (and the
    /// `.env` cascade) through the config system, validate, connect.
    pub fn for_root() -> DatabaseSetup {
        DatabaseSetup
    }
}

/// The configured form of [`DatabaseModule`]. Loads its config through
/// [`ConfigModule::for_feature`], then builds the pool and installs the request
/// layers.
pub struct DatabaseSetup;

impl DynamicModule for DatabaseSetup {
    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        install_request_layers(builder)
    }

    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::for_feature::<DatabaseConfig>().collect(builder);
        builder.provide_factory::<DatabaseConnection, _, _>(|container| async move {
            let config = container
                .get::<DatabaseConfig>()
                .expect("DatabaseConfig is loaded by ConfigModule::for_feature");
            connect(&config).await
        })
    }
}

/// Build the pool from a resolved config. The URL may carry credentials, so it is
/// never logged.
async fn connect(config: &DatabaseConfig) -> anyhow::Result<DatabaseConnection> {
    if config.url.is_empty() {
        anyhow::bail!("NESTRS_DATABASE__URL must be set");
    }
    tracing::info!(target: "nestrs::orm", "connecting to database");
    Ok(Database::connect(config.connect_options()).await?)
}

/// Install the sync request layers shared by both wiring paths: the `DbContext`
/// HTTP interceptor (ambient executor + per-request transaction) and the
/// `WorkerDbContext as dyn JobContext` bridge (ambient `Repo` for jobs). Built
/// eagerly from the snapshot — the pool is a factory output present before the
/// register phase.
fn install_request_layers(builder: ContainerBuilder) -> ContainerBuilder {
    let builder = <crate::DbContext as nestrs_core::Discoverable>::register(builder);
    let snapshot = builder.snapshot();
    let job_context = crate::WorkerDbContext::from_container(&snapshot);
    builder.provide_dyn::<dyn nestrs_core::JobContext>(Arc::new(job_context))
}
