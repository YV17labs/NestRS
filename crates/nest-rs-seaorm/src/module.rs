//! [`DatabaseModule`] — the async-owned SeaORM connection. Always wired with
//! `DatabaseModule::for_root()`; routes config through
//! [`ConfigModule::for_feature`] and installs the request layers.

use std::sync::Arc;

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};
use sea_orm::{Database, DatabaseConnection};

use crate::config::DatabaseConfig;

/// Registers a `sea_orm::DatabaseConnection` and installs the
/// [`DbContext`](crate::DbContext) request interceptor.
pub struct DatabaseModule;

impl DatabaseModule {
    /// Configure the database. Pass `None` to load [`DatabaseConfig`] from
    /// `NESTRS_DATABASE__*`, or a `DatabaseConfig` to pin it in code (wins
    /// over the environment — handy for tests).
    pub fn for_root(config: impl Into<Option<DatabaseConfig>>) -> DatabaseSetup {
        DatabaseSetup {
            pinned: config.into(),
        }
    }
}

pub struct DatabaseSetup {
    pinned: Option<DatabaseConfig>,
}

impl DynamicModule for DatabaseSetup {
    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        install_request_layers(builder)
    }

    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        builder.provide_factory::<DatabaseConnection, _, _>(|container| async move {
            let config = container
                .get::<DatabaseConfig>()
                .expect("DatabaseConfig is resolved by ConfigModule::provide_feature");
            connect(&config).await
        })
    }
}

/// The URL may carry credentials, so it is never logged.
async fn connect(config: &DatabaseConfig) -> anyhow::Result<DatabaseConnection> {
    if config.url.is_empty() {
        anyhow::bail!("NESTRS_DATABASE__URL must be set");
    }
    tracing::info!(target: "nest_rs::orm", "connecting to database");
    Ok(Database::connect(config.connect_options()).await?)
}

/// Install the sync request layers: the `DbContext` HTTP interceptor and the
/// `WorkerDbContext as dyn JobContext` bridge for jobs. Built eagerly from the
/// snapshot — the pool is a factory output present before the register phase.
fn install_request_layers(builder: ContainerBuilder) -> ContainerBuilder {
    let builder = <crate::DbContext as nest_rs_core::Discoverable>::register(builder);
    let snapshot = builder.snapshot();
    let job_context = crate::WorkerDbContext::from_container(&snapshot);
    builder.provide_dyn::<dyn nest_rs_worker::JobContext>(Arc::new(job_context))
}
