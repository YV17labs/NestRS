//! [`DatabaseConfig`] — connection settings for [`DatabaseModule`]. The
//! `from_env` mapping below is the single source of truth for which
//! `NESTRS_DATABASE__*` variable feeds each field.

use std::time::Duration;

use nest_rs_config::{Config, ConfigService, Result, config};
use sea_orm::ConnectOptions;
use validator::Validate;

#[config(namespace = "database")]
#[derive(Clone, Debug, Default, Validate)]
pub struct DatabaseConfig {
    /// e.g. `postgres://user:pass@host/db`. Empty aborts the build.
    pub url: String,
    pub max_connections: Option<u32>,
    pub min_connections: Option<u32>,
    pub connect_timeout_secs: Option<u64>,
    pub sqlx_logging: bool,
    /// Retry a mutating request's transaction when it fails with a Postgres
    /// `40001` (`serialization_failure`), `40P01` (`deadlock_detected`), or
    /// the MySQL/SQL-Server analogs (`1213`, `1205`). Off by default — opt
    /// in when the app uses serializable isolation and accepts the extra
    /// latency on contention. The interceptor can only retry at the commit
    /// boundary; for handler-time conflicts use
    /// [`retry_on_conflict`](crate::retry::retry_on_conflict) at the
    /// service's programmatic transaction boundary.
    /// `NESTRS_DATABASE__RETRY_SERIALIZATION_CONFLICTS`.
    pub retry_serialization_conflicts: bool,
}

impl Config for DatabaseConfig {
    fn from_env(env: &ConfigService) -> Result<Self> {
        Ok(Self {
            url: env.get("URL").unwrap_or_default(), //                NESTRS_DATABASE__URL
            max_connections: env.parse("MAX_CONNECTIONS")?, //         NESTRS_DATABASE__MAX_CONNECTIONS
            min_connections: env.parse("MIN_CONNECTIONS")?, //         NESTRS_DATABASE__MIN_CONNECTIONS
            connect_timeout_secs: env.parse("CONNECT_TIMEOUT_SECS")?, //NESTRS_DATABASE__CONNECT_TIMEOUT_SECS
            sqlx_logging: env.flag("SQLX_LOGGING", false)?, //         NESTRS_DATABASE__SQLX_LOGGING (else false)
            retry_serialization_conflicts: env
                .flag("RETRY_SERIALIZATION_CONFLICTS", false)?, //     NESTRS_DATABASE__RETRY_SERIALIZATION_CONFLICTS
        })
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
            retry_serialization_conflicts: false,
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
        assert!(!opts.get_sqlx_logging(), "noisy by default would spam prod logs");
    }

    #[test]
    fn retry_serialization_conflicts_defaults_off() {
        let cfg = DatabaseConfig::default();
        assert!(
            !cfg.retry_serialization_conflicts,
            "retry must default off — never change behaviour silently",
        );
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_env<R>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> R) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for (k, v) in vars {
            match v {
                Some(value) => unsafe { std::env::set_var(k, value) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
        let out = f();
        for (k, _) in vars {
            unsafe { std::env::remove_var(k) };
        }
        out
    }

    #[test]
    fn from_env_reads_url_and_pool_bounds() {
        with_env(
            &[
                ("NESTRS_DATABASE__URL", Some("postgres://u@h/d")),
                ("NESTRS_DATABASE__MAX_CONNECTIONS", Some("25")),
                ("NESTRS_DATABASE__MIN_CONNECTIONS", Some("2")),
                ("NESTRS_DATABASE__CONNECT_TIMEOUT_SECS", Some("12")),
                ("NESTRS_DATABASE__SQLX_LOGGING", Some("true")),
                ("NESTRS_DATABASE__RETRY_SERIALIZATION_CONFLICTS", Some("true")),
            ],
            || {
                let cfg =
                    DatabaseConfig::from_env(&ConfigService::for_namespace("database")).expect("ok");
                assert_eq!(cfg.url, "postgres://u@h/d");
                assert_eq!(cfg.max_connections, Some(25));
                assert_eq!(cfg.min_connections, Some(2));
                assert_eq!(cfg.connect_timeout_secs, Some(12));
                assert!(cfg.sqlx_logging);
                assert!(cfg.retry_serialization_conflicts);
            },
        );
    }

    #[test]
    fn from_env_defaults_to_empty_url_and_no_bounds() {
        with_env(
            &[
                ("NESTRS_DATABASE__URL", None),
                ("NESTRS_DATABASE__MAX_CONNECTIONS", None),
                ("NESTRS_DATABASE__MIN_CONNECTIONS", None),
                ("NESTRS_DATABASE__CONNECT_TIMEOUT_SECS", None),
                ("NESTRS_DATABASE__SQLX_LOGGING", None),
                ("NESTRS_DATABASE__RETRY_SERIALIZATION_CONFLICTS", None),
            ],
            || {
                let cfg =
                    DatabaseConfig::from_env(&ConfigService::for_namespace("database")).expect("ok");
                // Empty URL ⇒ module-level `for_root` aborts with a clear message.
                assert!(cfg.url.is_empty());
                assert!(cfg.max_connections.is_none());
                assert!(!cfg.sqlx_logging, "off by default — never noisy in prod");
                assert!(!cfg.retry_serialization_conflicts, "retry off by default");
            },
        );
    }
}
