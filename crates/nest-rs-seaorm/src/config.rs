//! [`DatabaseConfig`] — connection settings for [`DatabaseModule`]. The
//! `from_env` mapping below is the single source of truth for which
//! `NESTRS_DATABASE__*` variable feeds each field.

use std::time::Duration;

use nest_rs_config::{Config, ConfigService, Result, config};
use sea_orm::ConnectOptions;
use validator::Validate;

/// Connection settings for [`DatabaseModule`](crate::DatabaseModule). Every
/// field is settable via a `NESTRS_DATABASE__*` env var (see `from_env`) or
/// pinned through [`DatabaseModule::for_root`](crate::DatabaseModule::for_root).
#[config(namespace = "database")]
#[derive(Clone, Default, Validate)]
pub struct DatabaseConfig {
    /// e.g. `postgres://user:pass@host/db`. Empty aborts the build.
    pub url: String,
    /// Upper bound on pooled connections; `None` uses SeaORM's default.
    pub max_connections: Option<u32>,
    /// Lower bound on idle pooled connections; `None` uses SeaORM's default.
    pub min_connections: Option<u32>,
    /// How long to wait for a connection before failing; `None` uses the default.
    pub connect_timeout_secs: Option<u64>,
    /// Log every statement SeaORM issues. Off in production — chatty and leaks
    /// query shapes into logs.
    pub sqlx_logging: bool,
    /// Tag a mutating request's commit-time conflict — Postgres `40001`
    /// (`serialization_failure`), `40P01` (`deadlock_detected`), or the
    /// MySQL/SQL-Server analogs (`1213`, `1205`) — as a structured `warn` on
    /// `nest_rs::orm`, so contention is distinguishable from a generic commit
    /// error in the logs. Off by default; the response fails closed either way.
    ///
    /// This observes, it does not retry: replaying a conflict means re-running
    /// the whole handler, and a handler may already have emitted a
    /// non-transactional side effect (a queued job, an event, an object write).
    /// Retrying is the service's call, at a boundary it knows is replayable —
    /// [`retry_on_conflict`](crate::retry::retry_on_conflict).
    /// `NESTRS_DATABASE__OBSERVE_SERIALIZATION_CONFLICTS`.
    pub observe_serialization_conflicts: bool,
}

impl Config for DatabaseConfig {
    fn from_env(env: &ConfigService) -> Result<Self> {
        Ok(Self {
            url: env.get("URL").unwrap_or_default(), //                NESTRS_DATABASE__URL
            max_connections: env.parse("MAX_CONNECTIONS")?, //         NESTRS_DATABASE__MAX_CONNECTIONS
            min_connections: env.parse("MIN_CONNECTIONS")?, //         NESTRS_DATABASE__MIN_CONNECTIONS
            connect_timeout_secs: env.parse("CONNECT_TIMEOUT_SECS")?, //NESTRS_DATABASE__CONNECT_TIMEOUT_SECS
            sqlx_logging: env.flag("SQLX_LOGGING", false)?, //         NESTRS_DATABASE__SQLX_LOGGING (else false)
            observe_serialization_conflicts: env.flag("OBSERVE_SERIALIZATION_CONFLICTS", false)?, //     NESTRS_DATABASE__OBSERVE_SERIALIZATION_CONFLICTS
        })
    }
}

impl std::fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("url", &"<redacted>")
            .field("max_connections", &self.max_connections)
            .field("min_connections", &self.min_connections)
            .field("connect_timeout_secs", &self.connect_timeout_secs)
            .field("sqlx_logging", &self.sqlx_logging)
            .field(
                "observe_serialization_conflicts",
                &self.observe_serialization_conflicts,
            )
            .finish()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned(url: &str) -> DatabaseConfig {
        DatabaseConfig {
            url: url.into(),
            ..Default::default()
        }
    }

    #[test]
    fn connect_options_carries_url() {
        let opts = pinned("postgres://localhost/app").connect_options();
        assert_eq!(opts.get_url(), "postgres://localhost/app");
    }

    #[test]
    fn connect_options_omits_pool_bounds_by_default() {
        let opts = pinned("postgres://localhost/app").connect_options();
        assert_eq!(opts.get_max_connections(), None);
        assert_eq!(opts.get_min_connections(), None);
        assert_eq!(opts.get_connect_timeout(), None);
    }

    #[test]
    fn connect_options_propagates_pool_bounds_when_set() {
        let opts = DatabaseConfig {
            url: "postgres://localhost/app".into(),
            max_connections: Some(50),
            min_connections: Some(5),
            connect_timeout_secs: Some(8),
            sqlx_logging: true,
            observe_serialization_conflicts: false,
        }
        .connect_options();
        assert_eq!(opts.get_max_connections(), Some(50));
        assert_eq!(opts.get_min_connections(), Some(5));
        assert_eq!(opts.get_connect_timeout(), Some(Duration::from_secs(8)));
        assert!(opts.get_sqlx_logging());
    }

    #[test]
    fn connect_options_disables_sqlx_logging_by_default() {
        let opts = pinned("postgres://localhost/app").connect_options();
        assert!(
            !opts.get_sqlx_logging(),
            "noisy by default would spam prod logs"
        );
    }

    #[test]
    fn observe_serialization_conflicts_defaults_off() {
        let cfg = DatabaseConfig::default();
        assert!(
            !cfg.observe_serialization_conflicts,
            "retry must default off — never change behaviour silently",
        );
    }

    #[test]
    fn from_env_reads_url_and_pool_bounds() {
        let service = ConfigService::with_vars(
            "database",
            [
                ("NESTRS_DATABASE__URL", "postgres://u@h/d"),
                ("NESTRS_DATABASE__MAX_CONNECTIONS", "25"),
                ("NESTRS_DATABASE__MIN_CONNECTIONS", "2"),
                ("NESTRS_DATABASE__CONNECT_TIMEOUT_SECS", "12"),
                ("NESTRS_DATABASE__SQLX_LOGGING", "true"),
                ("NESTRS_DATABASE__OBSERVE_SERIALIZATION_CONFLICTS", "true"),
            ],
        );
        let cfg = DatabaseConfig::from_env(&service).expect("ok");
        assert_eq!(cfg.url, "postgres://u@h/d");
        assert_eq!(cfg.max_connections, Some(25));
        assert_eq!(cfg.min_connections, Some(2));
        assert_eq!(cfg.connect_timeout_secs, Some(12));
        assert!(cfg.sqlx_logging);
        assert!(cfg.observe_serialization_conflicts);
    }

    #[test]
    fn from_env_defaults_to_empty_url_and_no_bounds() {
        let cfg = DatabaseConfig::from_env(&ConfigService::with_vars("database", [])).expect("ok");
        // Empty URL ⇒ module-level `for_root` aborts with a clear message.
        assert!(cfg.url.is_empty());
        assert!(cfg.max_connections.is_none());
        assert!(!cfg.sqlx_logging, "off by default — never noisy in prod");
        assert!(
            !cfg.observe_serialization_conflicts,
            "conflict tagging off by default"
        );
    }
}
