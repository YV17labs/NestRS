//! [`DatabaseConfig`] — the connection settings for [`DatabaseModule`], a
//! namespaced `#[config]` loaded from `NESTRS_DATABASE__*` (and the `.env`
//! cascade). The single, typed source of truth, loaded env-driven by
//! `DatabaseModule::for_root()`.

use std::time::Duration;

use nestrs_config::config;
use sea_orm::ConnectOptions;
use serde::Deserialize;
use validator::Validate;

#[config(namespace = "database")]
#[derive(Clone, Debug, Default, Deserialize, Validate)]
pub struct DatabaseConfig {
    /// The database URL, e.g. `postgres://user:pass@host/db`
    /// (`NESTRS_DATABASE__URL`). Empty aborts the build with a clear message.
    #[serde(default)]
    pub url: String,
    /// Maximum pooled connections (`NESTRS_DATABASE__MAX_CONNECTIONS`; SeaORM
    /// default when unset).
    pub max_connections: Option<u32>,
    /// Minimum idle connections (`NESTRS_DATABASE__MIN_CONNECTIONS`).
    pub min_connections: Option<u32>,
    /// Connection-acquire timeout in whole seconds
    /// (`NESTRS_DATABASE__CONNECT_TIMEOUT_SECS`).
    pub connect_timeout_secs: Option<u64>,
    /// Log SQL via SeaORM's `sqlx` logging (`NESTRS_DATABASE__SQLX_LOGGING`).
    #[serde(default)]
    pub sqlx_logging: bool,
}

impl DatabaseConfig {
    pub(crate) fn connect_options(&self) -> ConnectOptions {
        let mut opts = ConnectOptions::new(self.url.clone());
        if let Some(n) = self.max_connections {
            opts.max_connections(n);
        }
        if let Some(n) = self.min_connections {
            opts.min_connections(n);
        }
        if let Some(secs) = self.connect_timeout_secs {
            opts.connect_timeout(Duration::from_secs(secs));
        }
        opts.sqlx_logging(self.sqlx_logging);
        opts
    }
}
