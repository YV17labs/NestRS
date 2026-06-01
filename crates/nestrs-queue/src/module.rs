//! `QueueModule` — owns the shared Redis [`QueueConnection`](crate::QueueConnection).
//!
//! Configured at its import site with **`QueueModule::for_root()`** (no bare form):
//! it routes the load through [`ConfigModule::for_feature`] (`NESTRS_QUEUE__*` +
//! the `.env` cascade) and registers a [`QueueConnection`].
//!
//! The connection is async, so it is built in the **collect phase** (a queued
//! factory `await`ed before the module tree is wired) — so the `QueueWorker`
//! transport and every producer inject it regardless of import order.
//!
//! ```ignore
//! #[module(imports = [QueueModule::for_root(), AudioModule])]
//! pub struct AppModule;
//! ```

use nestrs_config::ConfigModule;
use nestrs_core::{ContainerBuilder, DynamicModule};

use crate::config::QueueConfig;
use crate::QueueConnection;

/// The queue module. Wire it with `QueueModule::for_root()` (env-driven).
pub struct QueueModule;

impl QueueModule {
    /// Env-driven: load [`QueueConfig`] from `NESTRS_QUEUE__*` (and the `.env`
    /// cascade) through the config system, then connect.
    pub fn for_root() -> QueueSetup {
        QueueSetup
    }
}

/// The configured form of [`QueueModule`]. Loads its config through
/// [`ConfigModule::for_feature`], then opens the connection.
pub struct QueueSetup;

impl DynamicModule for QueueSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::for_feature::<QueueConfig>().collect(builder);
        builder.provide_factory::<QueueConnection, _, _>(|container| async move {
            let config = container
                .get::<QueueConfig>()
                .expect("QueueConfig is loaded by ConfigModule::for_feature");
            QueueConnection::connect(&config.url).await
        })
    }
}
