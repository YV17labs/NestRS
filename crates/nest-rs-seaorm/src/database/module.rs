//! [`SeaOrmDatabaseModule`] — the async-owned SeaORM connection. Always wired with
//! `SeaOrmDatabaseModule::for_root()`; routes config through
//! [`ConfigModule::for_feature`] and installs the request layers.

use std::sync::Arc;

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};
use sea_orm::{Database, DatabaseConnection};

use crate::SeaOrmDatabaseConfig;

/// Registers a `sea_orm::DatabaseConnection` and installs the
/// `DbContext` request interceptor.
pub struct SeaOrmDatabaseModule;

impl SeaOrmDatabaseModule {
    /// Configure the database. Pass `None` to load [`SeaOrmDatabaseConfig`] from
    /// `NESTRS_DATABASE__*`, or a `SeaOrmDatabaseConfig` to pin as the base those
    /// variables overlay, per field.
    ///
    /// A pin is not a test hatch: the deployment's real environment still wins
    /// over it. A test that must not read the ambient environment seeds the
    /// value instead — `App::builder().provide(cfg)` short-circuits the factory.
    pub fn for_root(config: impl Into<Option<SeaOrmDatabaseConfig>>) -> SeaOrmDatabaseSetup {
        SeaOrmDatabaseSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`SeaOrmDatabaseModule::for_root`]. Queues the
/// async pool factory and installs the request layers when registered; a pinned
/// `SeaOrmDatabaseConfig` is the base `NESTRS_DATABASE__*` overlays, per field.
pub struct SeaOrmDatabaseSetup {
    pinned: Option<SeaOrmDatabaseConfig>,
}

impl DynamicModule for SeaOrmDatabaseSetup {
    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        install_boot_audits(install_request_layers(builder))
    }

    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        builder.provide_factory::<DatabaseConnection, _, _>(|container| async move {
            let config = container
                .get::<SeaOrmDatabaseConfig>()
                .expect("SeaOrmDatabaseConfig is resolved by ConfigModule::provide_feature");
            connect(&config).await
        })
    }
}

/// Open a standalone connection from `NESTRS_DATABASE__*`, resolving the same
/// [`SeaOrmDatabaseConfig`] the app's [`SeaOrmDatabaseModule`] uses. The single connector
/// for tools outside the DI container (`migrate`, `seed`) — a new config knob
/// reaches them without editing each binary.
pub async fn connect_from_env() -> anyhow::Result<DatabaseConnection> {
    use nest_rs_config::Config;
    let config = SeaOrmDatabaseConfig::load()?;
    connect(&config).await
}

/// The URL may carry credentials, so it is never logged.
async fn connect(config: &SeaOrmDatabaseConfig) -> anyhow::Result<DatabaseConnection> {
    if config.url.is_empty() {
        anyhow::bail!(
            "{} must be set",
            nest_rs_config::var_name("database", "URL")
        );
    }
    tracing::info!(
        target: crate::TARGET,
        max_connections = ?config.max_connections,
        "connecting to database"
    );
    Ok(Database::connect(config.connect_options()).await?)
}

/// Install the sync request layers: the `DbContext` HTTP interceptor and the
/// `WorkerDbContext as dyn JobContext` bridge for jobs. Built eagerly from the
/// snapshot — the pool is a factory output present before the register phase.
fn install_request_layers(builder: ContainerBuilder) -> ContainerBuilder {
    // The `DbContext` interceptor only exists with the `http` feature (it is the
    // HTTP request seam). Without it there is no HTTP layer to install — the
    // worker bridge below still applies.
    #[cfg(feature = "http")]
    let builder = <crate::DbContext as nest_rs_core::Discoverable>::register(builder);
    let snapshot = builder.snapshot();
    let job_context = crate::WorkerDbContext::from_container(&snapshot);
    builder.provide_dyn::<dyn nest_rs_worker::JobContext>(Arc::new(job_context))
}

/// Install the link-time invariant checks this import brings — providers that
/// need neither config nor pool, only what the decorators submitted, and that
/// refuse boot from `#[on_module_init]`.
///
/// Separate from the request layers above: these run once and touch no request.
/// The audits ride `SeaOrmDatabaseModule` because an app without it has no
/// `CrudService` to mis-wire; an app composing the ORM some other way calls the
/// audit directly (`audit_soft_delete_bindings`).
fn install_boot_audits(builder: ContainerBuilder) -> ContainerBuilder {
    <crate::soft_delete::SoftDeleteAudit as nest_rs_core::Discoverable>::register(builder)
}
