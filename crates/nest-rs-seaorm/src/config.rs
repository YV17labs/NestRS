use std::time::Duration;

use std::str::FromStr;

use nest_rs_config::{
    Bound, Config, ConfigError, ConfigService, DurationBounds, Floor, Namespaced, Result, config,
};
use sea_orm::ConnectOptions;
use sea_orm::sqlx::postgres::{PgConnectOptions, PgSslMode};

/// The acquire budget's range: sqlx adds it to an `Instant` unchecked, so the
/// ceiling keeps it short of a panic inside sqlx.
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

/// The statement bound's range: Postgres cancels a statement past it
/// (`statement_timeout`).
pub(crate) const STATEMENT_TIMEOUT: DurationBounds = DurationBounds::secs(
    "STATEMENT_TIMEOUT_SECS",
    "SeaOrmConfig::statement_timeout_secs",
    Floor::Units(Bound {
        count: 1,
        why: "Postgres cancels every statement that takes longer, and under a second it cancels \
              ones that only had to wait their turn",
    }),
    Bound {
        count: 60 * 60,
        why: "the bound is how long one statement holds its connection and its locks, and a \
              statement running an hour belongs to an operator's tool, not to the app",
    },
);

/// The statement bound when none is set: under the authentication guard's
/// 20 s net and the HTTP edge's default 30 s deadline.
const DEFAULT_STATEMENT_TIMEOUT: Duration = Duration::from_secs(15);

/// The acquire budget when none is set: under the authentication guard's 20 s
/// net and the HTTP edge's default 30 s deadline, so a dry pool answers with its
/// own error.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

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
    /// 1 to 3600; `None` waits 10 s. The boot refuses a budget at or past the
    /// net of a guard whose code reaches the pool — the authentication guard's
    /// 20 s — which is every guard's once
    /// [`SeaOrmDatabaseModule`](crate::SeaOrmDatabaseModule) binds `Repo`.
    pub connect_timeout_secs: Option<u64>,
    /// How long one statement runs before Postgres cancels it
    /// (`statement_timeout`), in whole seconds, from 1 to 3600; `None` waits
    /// 15 s. It bounds every statement of the app's pool, a job's `BEGIN` /
    /// `COMMIT` / `ROLLBACK` included, and the boot refuses it at or past the net
    /// of a guard whose code reaches the pool. The tools
    /// ([`connect_from_env`](crate::connect_from_env)) open without it.
    pub statement_timeout_secs: Option<u64>,
    /// Log every statement SeaORM issues. Off in production — chatty and leaks
    /// query shapes into logs.
    pub sqlx_logging: bool,
    /// Tag a mutating request's commit-time conflict — Postgres `40001`
    /// (`serialization_failure`), `40P01` (`deadlock_detected`), or the
    /// MySQL/SQL-Server analogs (`1213`, `1205`) — as a structured `warn` on
    /// `nest_rs::orm`. Off by default; the response fails closed either way.
    ///
    /// It never retries: a handler may have emitted a non-transactional side
    /// effect. Retry at a replayable boundary with
    /// [`retry_on_conflict`](crate::retry::retry_on_conflict).
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
            statement_timeout_secs: STATEMENT_TIMEOUT
                .read_optional(env, base.statement_timeout_secs.map(Duration::from_secs))?
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
            .field("statement_timeout_secs", &self.statement_timeout_secs)
            .field("sqlx_logging", &self.sqlx_logging)
            .field(
                "observe_serialization_conflicts",
                &self.observe_serialization_conflicts,
            )
            .finish()
    }
}

impl SeaOrmConfig {
    /// The app's pool: [`connect_options`](Self::connect_options) with every
    /// statement bounded, the bound refused outside its range.
    pub(crate) fn app_connect_options(&self) -> Result<ConnectOptions> {
        let mut opts = self.connect_options()?;
        let bound = match self.statement_timeout_secs {
            Some(secs) => STATEMENT_TIMEOUT.check(
                Self::NAMESPACE,
                STATEMENT_TIMEOUT.field(),
                Duration::from_secs(secs),
            )?,
            None => DEFAULT_STATEMENT_TIMEOUT,
        };
        opts.statement_timeout(bound);
        Ok(opts)
    }

    /// The TLS the pool's connections open with: `verify-full` — the server's
    /// certificate checked against the system's authorities, or `sslrootcert`,
    /// and its name against the host — when the URL names no `sslmode`, and
    /// `disable` when it says so.
    /// A mode that encrypts without verifying, or may (`allow`, `prefer`,
    /// `require`, `verify-ca`), is refused, naming the variable and never the
    /// URL, which carries the password.
    fn tls_mode(&self) -> Result<PgSslMode> {
        let refuse = |message: String| ConfigError::parse(self.url_variable(), message);
        let named = self.query(&["sslmode", "ssl-mode"]).is_some();
        #[expect(
            clippy::map_err_ignore,
            reason = "sqlx's parse error can quote the URL, which carries the password"
        )]
        let parsed = PgConnectOptions::from_str(&self.url)
            .map_err(|_| refuse("is not a Postgres URL".to_owned()))?;
        match parsed.get_ssl_mode() {
            PgSslMode::Prefer if !named => Ok(PgSslMode::VerifyFull),
            mode @ (PgSslMode::Disable | PgSslMode::VerifyFull) => Ok(mode),
            unverified => Err(refuse(format!(
                "asks Postgres for TLS without verifying its certificate (sslmode={}): write \
                 sslmode=verify-full, with sslrootcert naming the authority that signed the \
                 server's certificate, or sslmode=disable for a server without TLS",
                match unverified {
                    PgSslMode::Allow => "allow",
                    PgSslMode::Prefer => "prefer",
                    PgSslMode::Require => "require",
                    _ => "verify-ca",
                },
            ))),
        }
    }

    /// `<PREFIX>_SEAORM__URL`, the variable every refusal of the URL names.
    fn url_variable(&self) -> String {
        nest_rs_config::var_name(Self::NAMESPACE, "URL")
    }

    /// Whether the pool trusts the system's authorities: when the URL names no
    /// `sslrootcert`, or names libpq's `system` — sqlx would read neither, and
    /// trusts the authorities compiled into it otherwise.
    fn trusts_the_system(&self) -> bool {
        self.query(&["sslrootcert", "ssl-root-cert", "ssl-ca"])
            .is_none_or(|root| root == "system")
    }

    /// The value the URL's query gives the first of `keys` it names.
    fn query(&self, keys: &[&str]) -> Option<&str> {
        let (_, query) = self.url.split_once('?')?;
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            keys.contains(&key).then_some(value)
        })
    }

    /// What every connection this config opens shares: TLS verified
    /// (`verify-full`, against the system's authorities unless `sslrootcert`
    /// names a file) or off (`sslmode=disable`), and the pool bounds.
    ///
    /// The one constructor every path reaches — the app's pool,
    /// [`connect_from_env`](crate::connect_from_env), a tool or a test
    /// opening a connection of its own — so each refuses what the boot does:
    /// an empty URL, a mode that encrypts without verifying, and a connect
    /// budget outside its range, which a config built in code never had
    /// checked. A refusal names its variable, and never quotes the URL, which
    /// carries the password.
    pub fn connect_options(&self) -> Result<ConnectOptions> {
        if self.url.is_empty() {
            return Err(ConfigError::parse(
                self.url_variable(),
                format!(
                    "must be set, inline or through {}",
                    nest_rs_config::var_name(Self::NAMESPACE, "URL_FILE")
                ),
            ));
        }
        let mode = self.tls_mode()?;
        let budget = match self.connect_timeout_secs {
            Some(secs) => CONNECT_TIMEOUT.check(
                Self::NAMESPACE,
                CONNECT_TIMEOUT.field(),
                Duration::from_secs(secs),
            )?,
            None => DEFAULT_CONNECT_TIMEOUT,
        };
        let mut opts = ConnectOptions::new(self.url.clone());
        let system = self.trusts_the_system();
        opts.map_sqlx_postgres_opts(move |options| {
            let options = options.ssl_mode(mode);
            if system {
                options.ssl_root_cert_from_pem(nest_rs_config::system_authorities().to_vec())
            } else {
                options
            }
        });
        if let Some(n) = self.max_connections {
            opts.max_connections(n);
        }
        if let Some(n) = self.min_connections {
            opts.min_connections(n);
        }
        opts.connect_timeout(budget);
        opts.sqlx_logging(self.sqlx_logging);
        Ok(opts)
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
    fn connect_options_refuses_a_mode_that_encrypts_without_verifying() {
        for unverified in ["allow", "prefer", "require", "verify-ca"] {
            let refused = pinned(&format!(
                "postgres://app:s3cret@db/app?sslmode={unverified}"
            ))
            .connect_options()
            .expect_err(unverified)
            .to_string();
            assert!(
                refused.starts_with(&format!(
                    "invalid value for {}",
                    nest_rs_config::var_name("seaorm", "URL")
                )) && refused.contains(&format!("sslmode={unverified}"))
                    && refused.contains("verify-full")
                    && refused.contains("disable")
                    && !refused.contains("s3cret"),
                "{refused}"
            );
        }
        for url in [
            "postgres://app:s3cret@db/app",
            "postgres://app:s3cret@db/app?sslmode=disable",
            "postgres://app:s3cret@db/app?sslmode=verify-full&sslrootcert=/ca.pem",
        ] {
            assert!(pinned(url).connect_options().is_ok(), "{url}");
        }
    }

    #[test]
    fn connect_options_refuses_what_is_not_a_postgres_url_without_quoting_it() {
        for (url, said) in [
            ("", "must be set"),
            ("postgres://app:s3cret@db:port/app", "is not a Postgres URL"),
        ] {
            let refused = pinned(url).connect_options().expect_err(url).to_string();
            assert!(
                refused.contains(&nest_rs_config::var_name("seaorm", "URL"))
                    && refused.contains(said)
                    && !refused.contains("s3cret"),
                "{refused}"
            );
        }
    }

    #[test]
    fn tls_is_verified_or_off_and_verified_unless_the_url_says_otherwise() {
        use sea_orm::sqlx::postgres::PgSslMode;
        for (url, mode) in [
            ("postgres://db/app", PgSslMode::VerifyFull),
            (
                "postgres://db/app?application_name=x",
                PgSslMode::VerifyFull,
            ),
            (
                "postgres://db/app?sslmode=verify-full&sslrootcert=/ca.pem",
                PgSslMode::VerifyFull,
            ),
            ("postgres://db/app?sslmode=disable", PgSslMode::Disable),
        ] {
            assert!(
                matches!(pinned(url).tls_mode(), Ok(found) if std::mem::discriminant(&found) == std::mem::discriminant(&mode)),
                "{url}"
            );
        }
    }

    #[test]
    fn the_pool_trusts_the_system_unless_the_url_names_an_authority_file() {
        for (url, system) in [
            ("postgres://db/app", true),
            ("postgres://db/app?sslmode=verify-full", true),
            (
                "postgres://db/app?sslmode=verify-full&sslrootcert=system",
                true,
            ),
            (
                "postgres://db/app?sslmode=verify-full&sslrootcert=/ca.pem",
                false,
            ),
            ("postgres://db/app?ssl-ca=/ca.pem", false),
        ] {
            assert_eq!(pinned(url).trusts_the_system(), system, "{url}");
        }
    }

    #[test]
    fn connect_options_carries_url() {
        let opts = pinned("postgres://localhost/app")
            .connect_options()
            .expect("a verified URL");
        assert_eq!(opts.get_url(), "postgres://localhost/app");
    }

    #[test]
    fn connect_options_omits_pool_bounds_by_default() {
        let opts = pinned("postgres://localhost/app")
            .connect_options()
            .expect("a verified URL");
        assert_eq!(opts.get_max_connections(), None);
        assert_eq!(opts.get_min_connections(), None);
    }

    #[test]
    fn the_apps_pool_bounds_its_statements_and_the_tools_do_not() {
        let config = pinned("postgres://localhost/app");
        assert_eq!(
            config
                .app_connect_options()
                .expect("a verified URL")
                .get_statement_timeout(),
            Some(DEFAULT_STATEMENT_TIMEOUT)
        );
        assert_eq!(
            config
                .connect_options()
                .expect("a verified URL")
                .get_statement_timeout(),
            None
        );
        let pinned = SeaOrmConfig {
            statement_timeout_secs: Some(3),
            ..config
        };
        assert_eq!(
            pinned
                .app_connect_options()
                .expect("a bound in range")
                .get_statement_timeout(),
            Some(Duration::from_secs(3))
        );
    }

    #[test]
    fn the_default_statement_bound_sits_below_every_net_a_query_runs_under() {
        assert!(DEFAULT_STATEMENT_TIMEOUT < nest_rs_authn::AUTHENTICATE_TIMEOUT);
        let request = nest_rs_http::HttpConfig::default()
            .request_timeout
            .expect("the edge bounds a request by default");
        assert!(DEFAULT_STATEMENT_TIMEOUT < request);
        assert!(DEFAULT_STATEMENT_TIMEOUT < nest_rs_worker::JOB_TIMEOUT);
    }

    #[test]
    fn the_default_budget_sits_below_every_net_a_query_runs_under() {
        let budget = pinned("postgres://localhost/app")
            .connect_options()
            .expect("a verified URL")
            .get_connect_timeout()
            .expect("a budget of the framework's, not sqlx's 30 s");
        assert!(budget < nest_rs_authn::AUTHENTICATE_TIMEOUT, "{budget:?}");
        let request = nest_rs_http::HttpConfig::default()
            .request_timeout
            .expect("the edge bounds a request by default");
        assert!(budget < request, "{budget:?}");
        assert!(budget < nest_rs_worker::JOB_TIMEOUT, "{budget:?}");
    }

    #[test]
    fn connect_options_propagates_pool_bounds_when_set() {
        let opts = SeaOrmConfig {
            url: "postgres://localhost/app".into(),
            max_connections: Some(50),
            min_connections: Some(5),
            connect_timeout_secs: Some(8),
            statement_timeout_secs: None,
            sqlx_logging: true,
            observe_serialization_conflicts: false,
        }
        .connect_options()
        .expect("bounds in range");
        assert_eq!(opts.get_max_connections(), Some(50));
        assert_eq!(opts.get_min_connections(), Some(5));
        assert_eq!(opts.get_connect_timeout(), Some(Duration::from_secs(8)));
        assert!(opts.get_sqlx_logging());
    }

    #[test]
    fn connect_options_disables_sqlx_logging_by_default() {
        let opts = pinned("postgres://localhost/app")
            .connect_options()
            .expect("a verified URL");
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
    fn a_budget_built_in_code_past_the_ceiling_is_refused_before_sqlx_sees_it() {
        let refused = SeaOrmConfig {
            connect_timeout_secs: Some(u64::MAX),
            ..pinned("postgres://nobody@127.0.0.1:1/none")
        }
        .connect_options()
        .expect_err("refused rather than handed to sqlx")
        .to_string();
        assert!(
            refused.contains(&nest_rs_config::var_name("seaorm", "CONNECT_TIMEOUT_SECS"))
                && refused.contains("above the 3600s it must be at most"),
            "{refused}"
        );
    }

    #[test]
    fn a_statement_bound_built_in_code_past_the_ceiling_is_refused_before_the_pool_opens() {
        let config = SeaOrmConfig {
            statement_timeout_secs: Some(60 * 60 + 1),
            ..pinned("postgres://nobody@127.0.0.1:1/none")
        };
        let refused = config
            .app_connect_options()
            .expect_err("refused rather than handed to Postgres")
            .to_string();
        assert!(
            refused.contains(&nest_rs_config::var_name(
                "seaorm",
                "STATEMENT_TIMEOUT_SECS"
            )) && refused.contains("above the 3600s it must be at most"),
            "{refused}"
        );
        assert!(
            config.connect_options().is_ok(),
            "a tool opens without the bound, so it is not refused one"
        );
    }

    #[test]
    fn from_env_defaults_to_empty_url_and_no_bounds() {
        let cfg =
            SeaOrmConfig::from_env(&ConfigService::with_vars("seaorm", []), Default::default())
                .expect("ok");
        assert!(cfg.url.is_empty());
        assert!(cfg.max_connections.is_none());
        assert!(!cfg.sqlx_logging, "off by default — never noisy in prod");
        assert!(
            !cfg.observe_serialization_conflicts,
            "conflict tagging off by default"
        );
    }
}
