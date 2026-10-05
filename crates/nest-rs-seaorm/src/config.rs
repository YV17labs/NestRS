//! [`SeaOrmConfig`] — the crate's one `#[config]`: the pool every SeaORM
//! binding shares. Namespace `seaorm`, read off the path like every other
//! config's: the pool is the crate's own subject, so it lives at the crate root
//! under the crate's word, and `<PREFIX>_SEAORM__URL` says which crate parses it.
//! The `from_env` mapping below is the single source of truth for which
//! `<PREFIX>_SEAORM__*` variable feeds each field.

use std::time::Duration;

use nest_rs_config::{Bound, Config, ConfigService, DurationBounds, Floor, Result, config};
use sea_orm::ConnectOptions;

/// The acquire budget's range, the variable that sets it, and why. SeaORM hands
/// it to sqlx as the pool's `acquire_timeout`: how long the boot waits for its
/// first connection, and every query after it for one from the pool. sqlx adds
/// it to an `Instant` unchecked, so a value past what a clock holds panicked the
/// boot inside sqlx naming nothing — the ceiling is what keeps every value the
/// boot accepts one the library accepts too.
pub(crate) const CONNECT_TIMEOUT: DurationBounds = DurationBounds::secs(
    "CONNECT_TIMEOUT_SECS",
    "SeaOrmConfig::connect_timeout_secs",
    Floor::Units(Bound {
        count: 1,
        why: "the pool gives up on a zero budget before any connection opens, so the boot fails \
              as a pool timeout against a database that answers",
    }),
    Bound {
        count: 60 * 60,
        why: "the budget is how long every query waits for a pooled connection, and past an hour \
              the request that asked for one has long been abandoned — a pool that cannot hand \
              one out sooner is down, not busy",
    },
);

/// Pool settings for [`SeaOrmModule`](crate::SeaOrmModule). Every field is
/// settable via a `<PREFIX>_SEAORM__*` env var (see `from_env`) or pinned through
/// [`SeaOrmModule::for_root`](crate::SeaOrmModule::for_root).
#[config(namespace = "seaorm")]
#[derive(Clone, Default)]
pub struct SeaOrmConfig {
    /// e.g. `postgres://user:pass@host/db`. Empty aborts the build.
    pub url: String,
    /// Upper bound on pooled connections; `None` uses SeaORM's default.
    pub max_connections: Option<u32>,
    /// Lower bound on idle pooled connections; `None` uses SeaORM's default.
    pub min_connections: Option<u32>,
    /// How long to wait for a connection before failing, in whole seconds, from
    /// 1 to 3600; `None` uses the default.
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
    /// `<PREFIX>_SEAORM__OBSERVE_SERIALIZATION_CONFLICTS`.
    pub observe_serialization_conflicts: bool,
}

impl Config for SeaOrmConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            url: env.get("URL")?.unwrap_or(base.url),
            max_connections: env.parse("MAX_CONNECTIONS")?.or(base.max_connections),
            min_connections: env.parse("MIN_CONNECTIONS")?.or(base.min_connections),
            connect_timeout_secs: CONNECT_TIMEOUT
                .read_optional(env, base.connect_timeout_secs.map(Duration::from_secs))?
                .map(|read| read.value.as_secs()),
            sqlx_logging: env.flag("SQLX_LOGGING", base.sqlx_logging)?,
            observe_serialization_conflicts: env.flag(
                "OBSERVE_SERIALIZATION_CONFLICTS",
                base.observe_serialization_conflicts,
            )?,
        })
    }
}

impl std::fmt::Debug for SeaOrmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeaOrmConfig")
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

impl SeaOrmConfig {
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

    fn pinned(url: &str) -> SeaOrmConfig {
        SeaOrmConfig {
            url: url.into(),
            ..Default::default()
        }
    }

    #[test]
    fn env_overrides_each_field_of_a_pinned_config() {
        use nest_rs_config::ConfigService;
        let cfg = SeaOrmConfig::from_env(
            &ConfigService::with_vars("seaorm", [("MAX_CONNECTIONS", "25")]),
            pinned("postgres://pinned/app"),
        )
        .expect("the overlay resolves");
        assert_eq!(cfg.max_connections, Some(25), "the env outranks the pin");
        assert_eq!(
            cfg.url, "postgres://pinned/app",
            "and the untouched pin survives",
        );
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
        let opts = SeaOrmConfig {
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
        let cfg = SeaOrmConfig::default();
        assert!(
            !cfg.observe_serialization_conflicts,
            "retry must default off — never change behaviour silently",
        );
    }

    #[test]
    fn from_env_reads_url_and_pool_bounds() {
        let service = ConfigService::with_vars(
            "seaorm",
            [
                ("URL", "postgres://u@h/d"),
                ("MAX_CONNECTIONS", "25"),
                ("MIN_CONNECTIONS", "2"),
                ("CONNECT_TIMEOUT_SECS", "12"),
                ("SQLX_LOGGING", "true"),
                ("OBSERVE_SERIALIZATION_CONFLICTS", "true"),
            ],
        );
        let cfg = SeaOrmConfig::from_env(&service, Default::default()).expect("ok");
        assert_eq!(cfg.url, "postgres://u@h/d");
        assert_eq!(cfg.max_connections, Some(25));
        assert_eq!(cfg.min_connections, Some(2));
        assert_eq!(cfg.connect_timeout_secs, Some(12));
        assert!(cfg.sqlx_logging);
        assert!(cfg.observe_serialization_conflicts);
    }

    /// A zero budget reached the pool as a literal zero, so every acquire timed
    /// out at once and the boot blamed the pool; it is refused naming the
    /// variable, from the environment or pinned in code, and unset stays unset.
    #[test]
    fn a_zero_connect_timeout_is_refused_from_either_side() {
        let var = nest_rs_config::var_name("seaorm", "CONNECT_TIMEOUT_SECS");
        let from_env = SeaOrmConfig::from_env(
            &ConfigService::with_vars("seaorm", [("CONNECT_TIMEOUT_SECS", "0")]),
            Default::default(),
        )
        .expect_err("a zero budget is refused")
        .to_string();
        assert!(
            from_env.contains(&var) && from_env.contains("must be at least 1 second"),
            "{from_env}"
        );
        let pinned = SeaOrmConfig::from_env(
            &ConfigService::with_vars("seaorm", []),
            SeaOrmConfig {
                connect_timeout_secs: Some(0),
                ..pinned("postgres://localhost/app")
            },
        )
        .expect_err("a pinned zero budget is refused")
        .to_string();
        assert!(
            pinned.contains(&var)
                && pinned.contains("`SeaOrmConfig::connect_timeout_secs` set in code is 0ns"),
            "{pinned}"
        );
        let unset =
            SeaOrmConfig::from_env(&ConfigService::with_vars("seaorm", []), Default::default())
                .expect("unset is the library's default");
        assert_eq!(unset.connect_timeout_secs, None);
    }

    /// config-4r2: the top of the range passed the reader and panicked the boot
    /// inside sqlx, "overflow when adding duration to instant", naming no
    /// variable. Past the ceiling it is refused naming the variable, from either
    /// side.
    #[test]
    fn a_connect_timeout_past_the_ceiling_is_refused_from_either_side() {
        let var = nest_rs_config::var_name("seaorm", "CONNECT_TIMEOUT_SECS");
        for value in ["3601", "18446744073709551615"] {
            let from_env = SeaOrmConfig::from_env(
                &ConfigService::with_vars("seaorm", [("CONNECT_TIMEOUT_SECS", value)]),
                Default::default(),
            )
            .expect_err("past the ceiling")
            .to_string();
            assert!(
                from_env.contains(&var) && from_env.contains("must be at most 3600 seconds"),
                "{from_env}"
            );
        }
        let pinned = SeaOrmConfig::from_env(
            &ConfigService::with_vars("seaorm", []),
            SeaOrmConfig {
                connect_timeout_secs: Some(u64::MAX),
                ..pinned("postgres://localhost/app")
            },
        )
        .expect_err("a pinned budget past the ceiling is refused")
        .to_string();
        assert!(
            pinned.contains(&var) && pinned.contains("above the 3600s it must be at most"),
            "{pinned}"
        );
    }

    #[test]
    fn from_env_defaults_to_empty_url_and_no_bounds() {
        let cfg =
            SeaOrmConfig::from_env(&ConfigService::with_vars("seaorm", []), Default::default())
                .expect("ok");
        // Empty URL ⇒ `SeaOrmModule::for_root` aborts naming the variable.
        assert!(cfg.url.is_empty());
        assert!(cfg.max_connections.is_none());
        assert!(!cfg.sqlx_logging, "off by default — never noisy in prod");
        assert!(
            !cfg.observe_serialization_conflicts,
            "conflict tagging off by default"
        );
    }
}
