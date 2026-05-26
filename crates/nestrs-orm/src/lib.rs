//! SeaORM database module for nestrs — the `@nestjs/typeorm` analog.
//!
//! A database pool is built asynchronously, which a synchronous
//! [`Module`](nestrs_core::Module) cannot do. So [`DatabaseModule`] is a
//! [`DynamicModule`](nestrs_core::DynamicModule) that owns its connection in the
//! **collect phase**: declared in `#[module(imports = [...])]` like any other
//! module, it queues a factory that [`AppBuilder::build`](nestrs_core::AppBuilder::build)
//! `await`s before providers are built. The connection logic and its options
//! live here, not in `main`. The pool is registered as a
//! `sea_orm::DatabaseConnection`, injectable anywhere as `Arc<DatabaseConnection>`:
//!
//! ```ignore
//! #[module(imports = [
//!     DatabaseModule::for_root(DatabaseOptions {
//!         url: std::env::var("DATABASE_URL").unwrap_or_default(),
//!         ..Default::default()
//!     }),
//!     UsersModule,
//! ])]
//! pub struct AppModule;
//! ```

use std::time::Duration;

use nestrs_core::{ContainerBuilder, DynamicModule};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};

/// Connection settings for [`DatabaseModule`], written as a struct literal at
/// the import site (like `GraphqlOptions`/`OpenApiOptions`). Set `url` — usually
/// from the environment — and take the pool-tuning fields from [`Default`]:
///
/// ```ignore
/// DatabaseOptions {
///     url: std::env::var("DATABASE_URL").unwrap_or_default(),
///     ..Default::default()
/// }
/// ```
///
/// An empty `url` aborts the build at the connect factory (`DATABASE_URL must be
/// set`), so a missing variable fails fast with a clear message.
#[derive(Clone, Debug, Default)]
pub struct DatabaseOptions {
    /// The database URL, e.g. `postgres://user:pass@host/db`.
    pub url: String,
    /// Maximum pooled connections (SeaORM default when `None`).
    pub max_connections: Option<u32>,
    /// Minimum idle connections to keep (SeaORM default when `None`).
    pub min_connections: Option<u32>,
    /// Timeout for acquiring a connection (SeaORM default when `None`).
    pub connect_timeout: Option<Duration>,
    /// Log SQL statements via SeaORM's `sqlx` logging. Default `false`.
    pub sqlx_logging: bool,
}

impl DatabaseOptions {
    fn connect_options(&self) -> ConnectOptions {
        let mut opts = ConnectOptions::new(self.url.clone());
        if let Some(n) = self.max_connections {
            opts.max_connections(n);
        }
        if let Some(n) = self.min_connections {
            opts.min_connections(n);
        }
        if let Some(d) = self.connect_timeout {
            opts.connect_timeout(d);
        }
        opts.sqlx_logging(self.sqlx_logging);
        opts
    }
}

/// The database module. List it in `#[module(imports = [...])]` via
/// [`for_root`](Self::for_root) — see the [crate docs](crate). It registers a
/// `sea_orm::DatabaseConnection`.
pub struct DatabaseModule;

impl DatabaseModule {
    /// Configure the connection at its import site. Returns a [`DynamicModule`]
    /// to list in `#[module(imports = [...])]`.
    pub fn for_root(options: DatabaseOptions) -> DatabaseSetup {
        DatabaseSetup { options }
    }
}

/// The configured form of [`DatabaseModule`], produced by
/// [`DatabaseModule::for_root`].
pub struct DatabaseSetup {
    options: DatabaseOptions,
}

impl DynamicModule for DatabaseSetup {
    // The pool is async, so it is queued in the collect phase and awaited before
    // providers are built — never in the synchronous `register`.
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let options = self.options.clone();
        builder.provide_factory::<DatabaseConnection, _, _>(move |_| async move {
            if options.url.is_empty() {
                anyhow::bail!("DATABASE_URL must be set");
            }
            // The URL may carry credentials, so it is never logged.
            tracing::info!(target: "nestrs::orm", "connecting to database");
            let conn = Database::connect(options.connect_options()).await?;
            Ok(conn)
        })
    }
}
