use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use nest_rs_config::__private::is_loopback;
use nest_rs_config::{
    Bound, ConfigError, CountBounds, DurationBounds, Floor, Namespaced, Result, Surface, config,
};
use nest_rs_core::Secret;
use sea_orm::ConnectOptions;
use sea_orm::sqlx::postgres::{PgConnectOptions, PgSslMode};

/// The acquire budget's range: sqlx adds it to an `Instant` unchecked, so the
/// ceiling keeps it short of a panic inside sqlx.
const CONNECT_TIMEOUT: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "the pool gives up on a zero budget before any connection opens, so the boot fails \
         as a pool timeout against a database that answers",
    )),
    Bound::new(
        60 * 60,
        "the budget is how long every query waits for a pooled connection, and past an hour \
         the request that asked for one has long been abandoned — a pool that cannot hand \
         one out sooner is down, not busy",
    ),
);

/// The statement bound's range: Postgres cancels a statement past it
/// (`statement_timeout`).
const STATEMENT_TIMEOUT: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "Postgres cancels every statement that takes longer, and under a second it cancels \
         ones that only had to wait their turn",
    )),
    Bound::new(
        60 * 60,
        "the bound is how long one statement holds its connection and its locks, and a \
         statement running an hour belongs to an operator's tool, not to the app",
    ),
);

/// The statement bound when none is set: under the authentication guard's
/// 18 s net and the HTTP edge's default 20 s deadline.
pub const DEFAULT_STATEMENT_TIMEOUT: Duration = Duration::from_secs(15);

// sqlx-postgres 0.9 quotes a malformed `.pgpass` line, password included, and
// an unknown URL parameter's value at `warn`.
nest_rs_core::inventory::submit! {
    nest_rs_core::__private::LibraryClamp::new(
        "sqlx_postgres",
        tracing::level_filters::LevelFilter::ERROR,
    )
}

// sea-orm 2.0's `connect` span holds its whole `ConnectOptions`, the URL's password
// included, and every span it opens is at `trace`.
nest_rs_core::inventory::submit! {
    nest_rs_core::__private::LibraryClamp::new(
        "sea_orm",
        tracing::level_filters::LevelFilter::DEBUG,
    )
}

/// The acquire budget when none is set: under the authentication guard's 18 s
/// net and the HTTP edge's default 20 s deadline, so a dry pool answers with its
/// own error.
pub(crate) const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// sqlx's own pool ceiling, which SeaORM keeps when it is handed none.
const DEFAULT_MAX_CONNECTIONS: u32 = 10;

/// What a Postgres server's own `max_connections` goes up to (`MAX_BACKENDS`), so
/// no pool can hold more of its connections.
const MOST_CONNECTIONS: u64 = 262_143;

const MAX_CONNECTIONS: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a pool of none hands out nothing, so every query waits out its connect budget against \
         a database that answers",
    ),
    Bound::new(
        MOST_CONNECTIONS,
        "a Postgres server admits no more connections than this whatever it is told, so a \
         larger pool is a slip the server would answer with `too many clients`",
    ),
);

const MIN_CONNECTIONS: CountBounds = CountBounds::new(
    Bound::new(
        0,
        "a pool keeping none idle opens each connection on demand",
    ),
    Bound::new(
        MOST_CONNECTIONS,
        "a Postgres server admits no more connections than this whatever it is told, and a \
         pool keeps at most its maximum idle",
    ),
);

/// libpq's variable naming the mode a URL leaves out, which sqlx reads.
const PGSSLMODE: &str = "PGSSLMODE";

/// libpq's variable naming the authority a URL leaves out, which sqlx reads.
pub(crate) const PGSSLROOTCERT: &str = "PGSSLROOTCERT";

/// The URL's spellings of the authority sqlx reads, libpq's first.
const ROOT_KEYS: &[&str] = &["sslrootcert", "ssl-root-cert", "ssl-ca"];

/// Each certificate file a connection reads — the authority, the client's
/// certificate and its key — as the URL spells it, libpq's first, and the
/// libpq variable naming it beside a URL naming none.
const TLS_FILES: [(&[&str], &str); 3] = [
    (ROOT_KEYS, PGSSLROOTCERT),
    (&["sslcert", "ssl-cert"], "PGSSLCERT"),
    (&["sslkey", "ssl-key"], "PGSSLKEY"),
];

/// What the server's certificate is verified to, libpq's root of verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrustRoot {
    /// The system's authorities: no root named, or libpq's `system`.
    System,
    /// What the URL's `sslrootcert` names.
    Url,
    /// What libpq's `PGSSLROOTCERT` names, beside a URL naming none.
    Variable,
}

/// What the boot says when a deployment reaches Postgres in plaintext.
pub(crate) const PLAINTEXT_OPEN: &str = "Postgres is reached in plaintext outside a dev profile: \
     its queries and their results travel unencrypted";

/// Pool settings for [`SeaOrmModule`](crate::SeaOrmModule). Every field is
/// settable via a `<PREFIX>_SEAORM__*` env var, its name in SCREAMING_SNAKE,
/// which the bare import reads, or pinned through
/// [`SeaOrmModule::for_root`](crate::SeaOrmModule::for_root).
#[config(namespace = "seaorm")]
#[derive(Clone, Debug)]
pub struct SeaOrmConfig {
    /// e.g. `postgres://user:pass@host/db` or `postgresql://…`, a [`Secret`] since
    /// it may carry the password. Empty or any other scheme aborts the build.
    /// `sslmode=disable` is plaintext, said once at `warn` in a deployed profile
    /// when the server is off this machine.
    pub url: Secret<String>,
    /// Upper bound on pooled connections, from 1 to 262143, Postgres's own
    /// ceiling; defaults to 10, sqlx's.
    #[config(bounds = MAX_CONNECTIONS)]
    pub max_connections: u32,
    /// Lower bound on idle pooled connections, from 0 to
    /// [`max_connections`](Self::max_connections), above which the boot refuses
    /// it; defaults to 0, sqlx's.
    #[config(bounds = MIN_CONNECTIONS)]
    pub min_connections: u32,
    /// How long to wait for a connection before failing, in whole seconds, from
    /// 1 to 3600; defaults to 10 s. The boot refuses a budget at or past the
    /// net of a guard whose code reaches the pool — the authentication guard's
    /// 18 s — which is every guard's once
    /// [`SeaOrmDatabaseModule`](crate::SeaOrmDatabaseModule) binds `Repo`.
    #[config(bounds = CONNECT_TIMEOUT)]
    pub connect_timeout: Duration,
    /// How long one statement runs before Postgres cancels it
    /// (`statement_timeout`), in whole seconds, from 1 to 3600; defaults to
    /// 15 s. It bounds every statement of the app's pool, a job's `BEGIN` /
    /// `COMMIT` / `ROLLBACK` included, and the boot refuses it at or past the net
    /// of a guard whose code reaches the pool. The tools
    /// ([`connect_from_env`](crate::connect_from_env)) open without it.
    #[config(bounds = STATEMENT_TIMEOUT)]
    pub statement_timeout: Duration,
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

impl Default for SeaOrmConfig {
    fn default() -> Self {
        Self {
            url: Secret::default(),
            max_connections: DEFAULT_MAX_CONNECTIONS,
            min_connections: 0,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            statement_timeout: DEFAULT_STATEMENT_TIMEOUT,
            sqlx_logging: false,
            observe_serialization_conflicts: false,
        }
    }
}

impl SeaOrmConfig {
    /// The app's pool: [`connect_options`](Self::connect_options) with every
    /// statement bounded, the bound refused outside its range.
    pub(crate) fn app_connect_options(&self) -> Result<ConnectOptions> {
        let mut opts = self.connect_options()?;
        let bound = Self::STATEMENT_TIMEOUT.check(Self::NAMESPACE, self.statement_timeout)?;
        opts.statement_timeout(bound);
        Ok(opts)
    }

    /// The options one connection outside the pool opens with — the URL, and
    /// the TLS every pool connection opens with — for the boot's proof, which
    /// dials one per attempt where the pool would retry in silence.
    pub(crate) fn probe_options(&self) -> Result<PgConnectOptions> {
        let tls = self.tls()?;
        Ok(tls(self.parsed()?))
    }

    /// The URL as sqlx reads it, or refused without quoting it: sqlx's parse
    /// error can quote it, and sqlx reads no scheme where sea-orm picks its
    /// driver by it, quoting the whole URL when it knows none.
    fn parsed(&self) -> Result<PgConnectOptions> {
        let url = self.url.expose_secret();
        let refused = || Self::refuse_url("is not a Postgres URL");
        #[expect(
            clippy::map_err_ignore,
            reason = "sqlx's parse error can quote the URL, which carries the password"
        )]
        let parsed = PgConnectOptions::from_str(url).map_err(|_| refused())?;
        // After the parse: `is_prefix_of` panics on a URL that does not read.
        if !sea_orm::DbBackend::Postgres.is_prefix_of(url) {
            return Err(refused());
        }
        Ok(parsed)
    }

    /// What every connection this config opens does with TLS: the mode
    /// [`tls_mode`](Self::tls_mode) allows, and the system's authorities where
    /// [`trust_root`](Self::trust_root) reads them — what sqlx read of the URL
    /// or of `PGSSLROOTCERT` stands otherwise.
    fn tls(&self) -> Result<impl Fn(PgConnectOptions) -> PgConnectOptions + Send + Sync + 'static> {
        let mode = self.tls_mode()?;
        if !matches!(mode, PgSslMode::Disable) {
            self.check_tls_files()?;
        }
        let system = self.trust_root() == TrustRoot::System;
        Ok(move |options: PgConnectOptions| {
            #[expect(
                clippy::disallowed_methods,
                reason = "the one call: `mode` is `tls_mode`'s, verified or explicitly off"
            )]
            let options = options.ssl_mode(mode);
            if system {
                options.ssl_root_cert_from_pem(nest_rs_config::system_authorities().to_vec())
            } else {
                options
            }
        })
    }

    /// The TLS the pool's connections open with: `verify-full` — the server's
    /// certificate checked against the system's authorities, or `sslrootcert`,
    /// and its name against the host — unless `sslmode=disable` in the URL, or
    /// libpq's `PGSSLMODE=disable` beside a URL naming no mode, turns it off.
    /// A mode that encrypts without verifying, or may (`allow`, `prefer`,
    /// `require`, `verify-ca`), is refused, naming the variable that set it and
    /// never the URL, which carries the password; `PGSSLMODE=prefer` reads as no
    /// mode named, being sqlx's own default.
    fn tls_mode(&self) -> Result<PgSslMode> {
        let named = self.query(&["sslmode", "ssl-mode"]).is_some();
        match self.parsed()?.get_ssl_mode() {
            PgSslMode::Prefer if !named => Ok(PgSslMode::VerifyFull),
            mode @ (PgSslMode::Disable | PgSslMode::VerifyFull) => Ok(mode),
            unverified if named => Err(Self::refuse_url(format!(
                "asks Postgres for TLS without verifying its certificate (sslmode={}): write \
                 sslmode=verify-full, with sslrootcert naming the authority that signed the \
                 server's certificate, or sslmode=disable for a server without TLS",
                unverified_mode(unverified),
            ))),
            unverified => Err(ConfigError::parse(
                PGSSLMODE,
                format!(
                    "asks Postgres for TLS without verifying its certificate (sslmode={}) beside \
                     {}, which names no mode: set it to verify-full, with sslrootcert in that URL \
                     naming the authority that signed the server's certificate, to disable for a \
                     server without TLS, or unset it",
                    unverified_mode(unverified),
                    Self::URL.var(Self::NAMESPACE),
                ),
            )),
        }
    }

    /// A refusal of the URL, naming `<PREFIX>_SEAORM__URL` and never the
    /// value, which carries the password.
    fn refuse_url(message: impl Into<String>) -> ConfigError {
        ConfigError::parse(Self::URL.var(Self::NAMESPACE), message)
    }

    /// What the server's certificate is verified to, as libpq reads it: the
    /// URL's `sslrootcert`, then `PGSSLROOTCERT`, and the system's authorities
    /// where neither names one or one names libpq's `system`. An empty value is
    /// none, and the URL's hides the variable, as libpq's connection parameter
    /// does.
    pub(crate) fn trust_root(&self) -> TrustRoot {
        match self.query(ROOT_KEYS).as_deref() {
            Some("system" | "") => TrustRoot::System,
            Some(_) => TrustRoot::Url,
            None => match libpq_var(PGSSLROOTCERT).as_deref() {
                None | Some("" | "system") => TrustRoot::System,
                Some(_) => TrustRoot::Variable,
            },
        }
    }

    /// Refuse a certificate file a connection would read and this process
    /// cannot, naming where it is set: every attempt would fail the same way,
    /// and sqlx says it as an I/O error the boot would retry for its budget.
    /// The file is read again at each connection, so a renewed one is taken.
    fn check_tls_files(&self) -> Result<()> {
        for (keys, var) in TLS_FILES {
            let names_no_root = |value: &str| keys == ROOT_KEYS && matches!(value, "system" | "");
            let named = self.query(keys);
            let path = match named.as_deref() {
                Some(value) if names_no_root(value) => continue,
                Some(value) => PathBuf::from(value),
                None => match libpq_var(var) {
                    Some(value) if names_no_root(&value) || is_inline_pem(&value) => continue,
                    Some(value) => PathBuf::from(value),
                    None => continue,
                },
            };
            let Err(error) = readable(&path) else {
                continue;
            };
            let why = format!(
                "this process cannot read ({}), so every connection would fail its TLS",
                nest_rs_core::error_message(&error)
            );
            return Err(match named {
                Some(_) => Self::refuse_url(format!("`{}` names a file {why}", keys[0])),
                None => ConfigError::parse(var, format!("names a file {why}")),
            });
        }
        Ok(())
    }

    /// The value the URL's query gives the last of `keys` it names, decoded:
    /// what sqlx reads, through the same parse, the last pair winning.
    fn query(&self, keys: &[&str]) -> Option<String> {
        let url = url::Url::parse(self.url.expose_secret()).ok()?;
        url.query_pairs()
            .filter(|(key, _)| keys.contains(&key.as_ref()))
            .last()
            .map(|(_, value)| value.into_owned())
    }

    /// What every connection this config opens shares: TLS verified
    /// (`verify-full`, against the system's authorities unless `sslrootcert`
    /// names a file, or libpq's `PGSSLROOTCERT` beside a URL naming none) or off
    /// (`sslmode=disable`, or libpq's `PGSSLMODE=disable` beside a URL naming no
    /// mode), and the pool bounds.
    ///
    /// The one constructor every path reaches — the app's pool,
    /// [`connect_from_env`](crate::connect_from_env), a tool or a test
    /// opening a connection of its own — so each refuses what the boot does:
    /// an empty URL, a mode that encrypts without verifying, a certificate file
    /// the URL or a libpq variable names that this process cannot read, and a
    /// connect budget or a pool bound outside its range, or a minimum above the
    /// maximum, which a config built in code never had checked. A refusal names its
    /// variable, and never quotes the URL, which carries the password.
    pub fn connect_options(&self) -> Result<ConnectOptions> {
        if self.url.expose_secret().is_empty() {
            return Err(ConfigError::unset(
                Self::NAMESPACE,
                Self::URL.key(),
                "`SeaOrmConfig` has no default URL, since the database it opens is the \
                 deployment's to name",
            ));
        }
        let tls = self.tls()?;
        let budget = Self::CONNECT_TIMEOUT.check(Self::NAMESPACE, self.connect_timeout)?;
        let (most, least) = self.pool_bounds()?;
        let mut opts = ConnectOptions::new(self.url.expose_secret().clone());
        opts.map_sqlx_postgres_opts(tls);
        opts.max_connections(most);
        opts.min_connections(least);
        opts.connect_timeout(budget);
        opts.sqlx_logging(self.sqlx_logging);
        Ok(opts)
    }

    /// The pool's maximum and minimum, each held to its range and the minimum
    /// to the maximum, where sqlx would clamp it in silence.
    fn pool_bounds(&self) -> Result<(u32, u32)> {
        let most = Self::MAX_CONNECTIONS.check(Self::NAMESPACE, self.max_connections)?;
        let least = Self::MIN_CONNECTIONS.check(Self::NAMESPACE, self.min_connections)?;
        if least > most {
            return Err(ConfigError::parse(
                Self::MIN_CONNECTIONS.var(Self::NAMESPACE),
                format!(
                    "must be at most {}: a pool keeps no more connections idle than it may hold, \
                     so sqlx would clamp the minimum in silence and keep fewer than asked for",
                    Self::MAX_CONNECTIONS.var(Self::NAMESPACE),
                ),
            ));
        }
        Ok((most, least))
    }
}

/// libpq's own variable `name`, read where sqlx reads it, so the two never
/// disagree: no prefix renames it.
#[expect(
    clippy::disallowed_methods,
    reason = "libpq's variable, which sqlx reads from the process itself"
)]
fn libpq_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// What sqlx reads as a PEM value rather than a file's path.
fn is_inline_pem(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.starts_with("-----BEGIN") && trimmed.ends_with("-----")
}

/// Whether this process can read the file at `path`, as each connection
/// does: a folder opens, and only its read fails. One byte is read, so no key
/// is copied.
fn readable(path: &Path) -> std::io::Result<()> {
    use std::io::Read as _;
    std::fs::File::open(path)?.read(&mut [0; 1]).map(drop)
}

/// A mode short of verifying, in the word `sslmode` writes it.
fn unverified_mode(mode: PgSslMode) -> &'static str {
    match mode {
        PgSslMode::Allow => "allow",
        PgSslMode::Prefer => "prefer",
        PgSslMode::Require => "require",
        _ => "verify-ca",
    }
}

/// One `warn` when a deployment opens `options`, what every connection of the
/// pool opens with, in plaintext off this machine: honoured, since `sslmode=disable`
/// or libpq's `PGSSLMODE` wrote it. A server on the loopback or a socket is
/// reached over no network, a sidecar proxy's common shape.
pub(crate) fn warn_when_open(options: &PgConnectOptions) {
    if matches!(options.get_ssl_mode(), PgSslMode::Disable)
        && crate::module::socket(options).is_none()
        && !is_loopback(options.get_host())
        && let Some(environment) = Surface::Open.opened_in_deployment()
    {
        tracing::warn!(
            target: crate::target::ORM,
            environment = environment.as_str(),
            endpoint = %crate::module::endpoint(options),
            "{PLAINTEXT_OPEN}",
        );
    }
}

#[cfg(test)]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
mod tests {
    use nest_rs_config::{Config, ConfigService};

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
        assert_eq!(cfg.max_connections, 25, "the env outranks the pin");
        assert_eq!(
            cfg.url.expose_secret(),
            "postgres://pinned/app",
            "and the untouched pin survives",
        );
    }

    #[test]
    fn debug_never_prints_the_url() {
        let printed = format!("{:?}", pinned("postgres://app:s3cret@db/app"));
        assert!(!printed.contains("s3cret"), "{printed}");
        assert!(printed.contains("url: <redacted>"), "{printed}");
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
        figment::Jail::expect_with(|jail| {
            jail.create_file("ca.pem", "")?;
            for url in [
                "postgres://app:s3cret@db/app",
                "postgres://app:s3cret@db/app?sslmode=disable",
                "postgres://app:s3cret@db/app?sslmode=verify-full&sslrootcert=ca.pem",
            ] {
                assert!(pinned(url).connect_options().is_ok(), "{url}");
            }
            Ok(())
        });
    }

    /// A deployment reaching Postgres in plaintext says so, by host and port and
    /// never the URL, which carries the password. A server on this machine, by
    /// its loopback or a socket, is reached over no network.
    #[test]
    fn a_deployment_reaching_postgres_in_plaintext_off_this_machine_is_said() {
        const DISABLED: &str = "postgres://app:s3cret@db.internal:5433/app?sslmode=disable";
        figment::Jail::expect_with(|jail| {
            jail.create_file("ca.pem", "")?;
            for (profile, url, endpoint) in [
                ("production", DISABLED, Some("db.internal:5433")),
                ("staging", DISABLED, Some("db.internal:5433")),
                ("", DISABLED, Some("db.internal:5433")),
                (
                    "production",
                    "postgres:///app?host=db&password=s3cret&sslmode=disable",
                    Some("db:5432"),
                ),
                ("development", DISABLED, None),
                ("test", DISABLED, None),
                ("production", "postgres://app:s3cret@db/app", None),
                (
                    "production",
                    "postgres://app:s3cret@db/app?sslmode=verify-full&sslrootcert=ca.pem",
                    None,
                ),
                (
                    "production",
                    "postgres://app:s3cret@127.0.0.1/app?sslmode=disable",
                    None,
                ),
                (
                    "production",
                    "postgres://app:s3cret@localhost/app?sslmode=disable",
                    None,
                ),
                (
                    "production",
                    "postgres://app:s3cret@[::1]/app?sslmode=disable",
                    None,
                ),
                (
                    "production",
                    "postgres:///app?host=/run/postgresql&password=s3cret&sslmode=disable",
                    None,
                ),
            ] {
                jail.set_env(nest_rs_config::Environment::var_name(), profile);
                let logs = nest_rs_testing::LogCapture::install();
                warn_when_open(&pinned(url).probe_options().expect(url));
                let lines = logs.find(crate::target::ORM, PLAINTEXT_OPEN);
                assert_eq!(
                    lines
                        .iter()
                        .filter_map(|line| line.field("endpoint"))
                        .collect::<Vec<_>>(),
                    endpoint.map(str::to_owned).into_iter().collect::<Vec<_>>(),
                    "{profile:?} {url}",
                );
                for line in lines {
                    assert_eq!(line.level, "warn");
                    assert_eq!(
                        line.field("environment").as_deref(),
                        Some(if profile.is_empty() {
                            "production"
                        } else {
                            profile
                        }),
                    );
                    assert!(!format!("{line:?}").contains("s3cret"), "{line:?}");
                }
            }
            Ok(())
        });
    }

    /// libpq's own variable turns TLS off beside a URL naming no mode, as the
    /// URL's parameter does, and is said as one.
    #[test]
    fn pgsslmode_disabling_tls_beside_a_url_naming_no_mode_is_said() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(nest_rs_config::Environment::var_name(), "production");
            jail.set_env("PGSSLMODE", "disable");
            let logs = nest_rs_testing::LogCapture::install();
            let probe = pinned("postgres://db.internal/app")
                .probe_options()
                .expect("a URL");
            assert!(matches!(probe.get_ssl_mode(), PgSslMode::Disable));
            warn_when_open(&probe);
            let line = logs.expect_one(crate::target::ORM, PLAINTEXT_OPEN);
            assert_eq!(line.field("endpoint").as_deref(), Some("db.internal:5432"));
            Ok(())
        });
    }

    /// libpq's `PGHOST` and `PGPORT` name what a URL leaves out, and read as sqlx
    /// dials them: a directory is a socket, as the URL's own `host=` is.
    #[test]
    fn pghost_reaches_the_line_as_what_it_dials() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(nest_rs_config::Environment::var_name(), "production");
            jail.set_env("PGPORT", "5433");
            for (host, endpoint) in [
                ("/run/postgresql", None),
                ("127.0.0.1", None),
                ("db.internal", Some("db.internal:5433")),
            ] {
                jail.set_env("PGHOST", host);
                let logs = nest_rs_testing::LogCapture::install();
                let probe = pinned("postgres:///app?sslmode=disable")
                    .probe_options()
                    .expect("a URL");
                warn_when_open(&probe);
                let lines = logs.find(crate::target::ORM, PLAINTEXT_OPEN);
                assert_eq!(
                    lines
                        .iter()
                        .filter_map(|line| line.field("endpoint"))
                        .collect::<Vec<_>>(),
                    endpoint.map(str::to_owned).into_iter().collect::<Vec<_>>(),
                    "{host}",
                );
            }
            Ok(())
        });
    }

    /// A mode libpq's variable set is refused naming that variable, the URL
    /// holding no such word; the URL's own word stands over it, and is the one
    /// named.
    #[test]
    fn a_pgsslmode_that_does_not_verify_is_refused_naming_it() {
        figment::Jail::expect_with(|jail| {
            let url = nest_rs_config::var_name("seaorm", "URL");
            for mode in ["allow", "require", "verify-ca"] {
                jail.set_env("PGSSLMODE", mode);
                let refused = pinned("postgres://app:s3cret@db.internal/app")
                    .connect_options()
                    .expect_err(mode)
                    .to_string();
                assert!(
                    refused.starts_with("invalid value for PGSSLMODE: ")
                        && refused.contains(&format!("(sslmode={mode})"))
                        && refused.contains(&format!("beside {url}, which names no mode"))
                        && !refused.contains("s3cret"),
                    "{refused}"
                );
                assert!(
                    pinned("postgres://db.internal/app?sslmode=verify-full")
                        .connect_options()
                        .is_ok(),
                    "{mode}",
                );
            }
            jail.set_env("PGSSLMODE", "verify-full");
            let refused = pinned("postgres://db.internal/app?sslmode=require")
                .connect_options()
                .expect_err("the URL's word")
                .to_string();
            assert!(
                refused.starts_with(&format!("invalid value for {url}: ")),
                "{refused}"
            );
            Ok(())
        });
    }

    #[test]
    fn connect_options_refuses_what_is_not_a_postgres_url_without_quoting_it() {
        for (url, said) in [
            ("", "must be set"),
            ("postgres://app:s3cret@db:port/app", "is not a Postgres URL"),
            ("postgress://app:s3cret@db/app", "is not a Postgres URL"),
            ("mysql://app:s3cret@db/app", "is not a Postgres URL"),
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

    /// libpq's order: the URL's `sslrootcert`, then `PGSSLROOTCERT`, then the
    /// system's; `system` and an empty value name none.
    #[test]
    fn the_root_of_verification_is_read_as_libpq_reads_it() {
        figment::Jail::expect_with(|jail| {
            for (variable, url, root) in [
                (None, "postgres://db/app", TrustRoot::System),
                (
                    None,
                    "postgres://db/app?sslmode=verify-full",
                    TrustRoot::System,
                ),
                (
                    None,
                    "postgres://db/app?sslrootcert=system",
                    TrustRoot::System,
                ),
                (None, "postgres://db/app?sslrootcert=", TrustRoot::System),
                (
                    None,
                    "postgres://db/app?sslrootcert=/ca.pem",
                    TrustRoot::Url,
                ),
                (None, "postgres://db/app?ssl-ca=/ca.pem", TrustRoot::Url),
                (Some("/ca.pem"), "postgres://db/app", TrustRoot::Variable),
                (Some("system"), "postgres://db/app", TrustRoot::System),
                (Some(""), "postgres://db/app", TrustRoot::System),
                (
                    Some("/ca.pem"),
                    "postgres://db/app?sslrootcert=system",
                    TrustRoot::System,
                ),
                (
                    Some("/ca.pem"),
                    "postgres://db/app?sslrootcert=/other.pem",
                    TrustRoot::Url,
                ),
            ] {
                match variable {
                    Some(value) => jail.set_env(PGSSLROOTCERT, value),
                    None => jail.clear_env(),
                }
                assert_eq!(pinned(url).trust_root(), root, "{variable:?}, {url}");
            }
            Ok(())
        });
    }

    /// Each file is checked where sqlx reads it: the last pair naming it in
    /// any spelling, key and value decoded as sqlx decodes them.
    #[test]
    fn the_query_is_read_as_sqlx_reads_it() {
        use sea_orm::sqlx::ConnectOptions as _;
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            for query in [
                "sslrootcert=/certs/ca.pem",
                "sslrootcert=%2Fcerts%2Fmy%20ca.pem",
                "sslrootcert=my+ca.pem",
                "sslrootcert=100%",
                "sslrootcert=%zz",
                "sslrootcert=%+1.pem",
                "sslrootcert=%25%2B1%+1.pem",
                "sslrootcert=gone.pem&sslrootcert=ca.pem",
                "sslrootcert=ca.pem&sslrootcert=gone.pem",
                "sslrootcert=ca.pem&ssl-ca=gone.pem",
                "ssl%72ootcert=ca.pem",
                "sslrootcert=ca.pem#gone.pem",
                "sslcert=client.pem&sslkey=client.key&ssl-cert=other.pem",
                "ssl%2Dkey=client.key",
            ] {
                let url = format!("postgres://db/app?{query}");
                let sqlx = PgConnectOptions::from_str(&url)
                    .expect("sqlx reads the URL")
                    .to_url_lossy();
                for (keys, _) in TLS_FILES {
                    let read = sqlx
                        .query_pairs()
                        .find(|(key, _)| key == keys[0])
                        .and_then(|(_, value)| value.strip_prefix("file: ").map(str::to_owned));
                    assert_eq!(pinned(&url).query(keys), read, "{query}: {}", keys[0]);
                }
            }
            Ok(())
        });
    }

    /// A mode the URL names under an encoded key is the URL's, as sqlx reads
    /// it, so its refusal names the URL and never libpq's `PGSSLMODE`.
    #[test]
    fn a_mode_named_under_an_encoded_key_is_the_urls() {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            let refused = pinned("postgres://db/app?ssl%2Dmode=require")
                .connect_options()
                .expect_err("sqlx reads `require`")
                .to_string();
            assert!(
                refused.starts_with(&format!(
                    "invalid value for {}: asks Postgres for TLS without verifying",
                    nest_rs_config::var_name("seaorm", "URL")
                )),
                "{refused}"
            );
            Ok(())
        });
    }

    /// A file `connect_options` would hand sqlx and this process cannot read —
    /// gone, or a folder, which opens and cannot be read — is refused naming
    /// where it is set, never with its path or the URL.
    #[test]
    fn a_certificate_file_that_cannot_be_read_is_refused_naming_where_it_is_set() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("ca.pem", "")?;
            jail.create_dir("folder")?;
            for unreadable in ["gone.pem", "folder"] {
                jail.clear_env();
                for (query, refused) in [
                    (format!("sslrootcert={unreadable}"), "`sslrootcert`"),
                    (format!("sslcert={unreadable}&sslkey=ca.pem"), "`sslcert`"),
                    (format!("ssl-key={unreadable}&sslcert=ca.pem"), "`sslkey`"),
                ] {
                    let url = format!("postgres://app:s3cret@db/app?{query}");
                    let said = pinned(&url).connect_options().expect_err(&url).to_string();
                    assert!(
                        said.starts_with(&format!(
                            "invalid value for {}: {refused} names a file this process cannot \
                             read",
                            nest_rs_config::var_name("seaorm", "URL")
                        )) && !said.contains(unreadable)
                            && !said.contains("s3cret"),
                        "{said}"
                    );
                }
                for var in [PGSSLROOTCERT, "PGSSLCERT", "PGSSLKEY"] {
                    jail.clear_env();
                    jail.set_env(var, unreadable);
                    let said = pinned("postgres://app:s3cret@db/app")
                        .connect_options()
                        .expect_err(var)
                        .to_string();
                    assert!(
                        said.starts_with(&format!(
                            "invalid value for {var}: names a file this process cannot read"
                        )),
                        "{var}={unreadable}: {said}"
                    );
                }
            }
            jail.clear_env();
            jail.set_env(PGSSLROOTCERT, "gone.pem");
            assert!(
                pinned("postgres://db/app?sslmode=disable")
                    .connect_options()
                    .is_ok(),
                "a connection without TLS reads no certificate"
            );
            jail.set_env(
                PGSSLROOTCERT,
                "-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----",
            );
            assert!(
                pinned("postgres://db/app").connect_options().is_ok(),
                "an inline PEM is read as no file"
            );
            Ok(())
        });
    }

    #[test]
    fn connect_options_carries_url() {
        let opts = pinned("postgres://localhost/app")
            .connect_options()
            .expect("a verified URL");
        assert_eq!(opts.get_url(), "postgres://localhost/app");
    }

    #[test]
    fn connect_options_hands_sqlx_its_own_pool_bounds_by_default() {
        let opts = pinned("postgres://localhost/app")
            .connect_options()
            .expect("a verified URL");
        assert_eq!(opts.get_max_connections(), Some(10));
        assert_eq!(opts.get_min_connections(), Some(0));
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
            statement_timeout: Duration::from_secs(3),
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
        assert!(DEFAULT_STATEMENT_TIMEOUT < nest_rs_core::JOB_TIMEOUT);
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
        assert!(budget < nest_rs_core::JOB_TIMEOUT, "{budget:?}");
    }

    #[test]
    fn connect_options_propagates_pool_bounds_when_set() {
        let opts = SeaOrmConfig {
            url: "postgres://localhost/app".into(),
            max_connections: 50,
            min_connections: 5,
            connect_timeout: Duration::from_secs(8),
            sqlx_logging: true,
            ..SeaOrmConfig::default()
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
        assert_eq!(cfg.url.expose_secret(), "postgres://u@h/d");
        assert_eq!(cfg.max_connections, 25);
        assert_eq!(cfg.min_connections, 2);
        assert_eq!(cfg.connect_timeout, Duration::from_secs(12));
        assert!(cfg.sqlx_logging);
        assert!(cfg.observe_serialization_conflicts);
    }

    /// A pool holds at least one connection and at most what a Postgres server
    /// admits, and keeps none to that many idle, from the environment and from
    /// a pin, naming the variable and its range and never the value it was given.
    #[test]
    fn the_pool_bounds_are_held_to_their_ranges_from_either_side() {
        for (key, raw, said) in [
            ("MAX_CONNECTIONS", "0", "must be at least 1 — "),
            ("MAX_CONNECTIONS", "262144", "must be at most 262143 — "),
            ("MIN_CONNECTIONS", "262144", "must be at most 262143 — "),
        ] {
            let var = nest_rs_config::var_name("seaorm", key);
            let refused = SeaOrmConfig::from_env(
                &ConfigService::with_vars("seaorm", [(key, raw)]),
                SeaOrmConfig::default(),
            )
            .expect_err("outside the range")
            .to_string();
            assert!(
                refused.contains(&var) && refused.contains(said),
                "{refused}"
            );
            assert!(!refused.contains("262144"), "{refused}");
        }
        let pinned = |max_connections, min_connections| SeaOrmConfig {
            max_connections,
            min_connections,
            ..SeaOrmConfig::default()
        };
        for (config, field, said) in [
            (
                pinned(0, 0),
                "max_connections",
                "below the 1 it must be at least",
            ),
            (
                pinned(262_144, 0),
                "max_connections",
                "above the 262143 it must be at most",
            ),
            (
                pinned(10, 262_144),
                "min_connections",
                "above the 262143 it must be at most",
            ),
        ] {
            let refused = SeaOrmConfig::from_env(&ConfigService::with_vars("seaorm", []), config)
                .expect_err("a pin outside the range")
                .to_string();
            assert!(
                refused.contains(&format!("`SeaOrmConfig::{field}` set in code"))
                    && refused.contains(said),
                "{refused}"
            );
        }
        let zero = SeaOrmConfig::from_env(
            &ConfigService::with_vars("seaorm", [("MIN_CONNECTIONS", "0")]),
            SeaOrmConfig::default(),
        )
        .expect("no idle connection is a pool that opens on demand");
        assert_eq!(zero.min_connections, 0);
    }

    /// sqlx clamps a minimum past the maximum in silence, so the pool would keep
    /// fewer idle than the deployment asked for: every path opening it refuses
    /// the pair, naming both variables and never their values.
    #[test]
    fn a_minimum_above_the_maximum_is_refused_where_every_path_opens_the_pool() {
        let config = SeaOrmConfig {
            max_connections: 4,
            min_connections: 7,
            ..pinned("postgres://localhost/app")
        };
        for refused in [
            config.connect_options().expect_err("the tools' path"),
            config.app_connect_options().expect_err("the app's pool"),
        ] {
            let refused = refused.to_string();
            assert!(
                refused.contains(&nest_rs_config::var_name("seaorm", "MIN_CONNECTIONS"))
                    && refused.contains(&nest_rs_config::var_name("seaorm", "MAX_CONNECTIONS")),
                "{refused}"
            );
            assert!(
                !refused.contains('7') && !refused.contains('4'),
                "{refused}"
            );
        }
        assert!(
            SeaOrmConfig {
                max_connections: 7,
                min_connections: 7,
                ..pinned("postgres://localhost/app")
            }
            .connect_options()
            .is_ok(),
            "a pool kept full is a pool"
        );
    }

    /// `connect_options` is the one constructor a config built in code reaches.
    #[test]
    fn pool_bounds_built_in_code_outside_their_ranges_are_refused_before_the_pool_opens() {
        for (config, var) in [
            (
                SeaOrmConfig {
                    max_connections: 0,
                    ..pinned("postgres://localhost/app")
                },
                "MAX_CONNECTIONS",
            ),
            (
                SeaOrmConfig {
                    max_connections: 262_144,
                    min_connections: 262_144,
                    ..pinned("postgres://localhost/app")
                },
                "MAX_CONNECTIONS",
            ),
        ] {
            let refused = config.connect_options().expect_err(var).to_string();
            assert!(
                refused.contains(&nest_rs_config::var_name("seaorm", var)),
                "{refused}"
            );
        }
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
                connect_timeout: Duration::ZERO,
                ..pinned("postgres://localhost/app")
            },
        )
        .expect_err("a pinned zero budget is refused")
        .to_string();
        assert!(
            pinned.contains(&var)
                && pinned.contains("`SeaOrmConfig::connect_timeout` set in code is 0ns"),
            "{pinned}"
        );
        let unset =
            SeaOrmConfig::from_env(&ConfigService::with_vars("seaorm", []), Default::default())
                .expect("unset keeps the default");
        assert_eq!(unset.connect_timeout, DEFAULT_CONNECT_TIMEOUT);
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
                connect_timeout: Duration::MAX,
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
            connect_timeout: Duration::MAX,
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
            statement_timeout: Duration::from_secs(60 * 60 + 1),
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
    fn from_env_defaults_to_empty_url_and_sqlx_pool_bounds() {
        let cfg =
            SeaOrmConfig::from_env(&ConfigService::with_vars("seaorm", []), Default::default())
                .expect("ok");
        assert!(cfg.url.expose_secret().is_empty());
        assert_eq!((cfg.max_connections, cfg.min_connections), (10, 0));
        assert!(!cfg.sqlx_logging, "off by default — never noisy in prod");
        assert!(
            !cfg.observe_serialization_conflicts,
            "conflict tagging off by default"
        );
    }

    /// sqlx-postgres quotes a malformed `.pgpass` line, password included, and
    /// an unknown connection parameter's value, both at `warn`; the statement
    /// log `sqlx_logging` turns on is `sqlx::query`'s, and stays open.
    #[test]
    fn sqlx_postgres_writes_only_its_errors_unless_a_filter_names_it() {
        use nest_rs_core::__written_under as written;

        for directive in ["info", "debug", "trace"] {
            assert!(
                !written!(directive, "sqlx_postgres::options::pgpass", WARN),
                "{directive}"
            );
            assert!(
                !written!(directive, "sqlx_postgres::options::parse", WARN),
                "{directive}"
            );
            assert!(
                written!(directive, "sqlx_postgres::advisory_lock", ERROR),
                "{directive}"
            );
            assert!(written!(directive, "sqlx::query", INFO), "{directive}");
        }
        assert!(written!(
            "info,sqlx_postgres=warn",
            "sqlx_postgres::options::pgpass",
            WARN
        ));
    }

    /// sea-orm's `connect` span holds its whole `ConnectOptions` at `trace`, the
    /// URL's password included, and an exporter ships a span's fields whatever
    /// the console prints. Its `debug` and above quote nothing.
    #[test]
    fn sea_orms_spans_open_only_under_a_filter_naming_it() {
        use nest_rs_core::__written_under as written;

        let opens = |directive: &str| {
            tracing::dispatcher::with_default(
                &nest_rs_core::__private::filter_dispatch(directive),
                || {
                    tracing::span_enabled!(
                        target: "sea_orm::driver::sqlx_postgres",
                        tracing::Level::TRACE
                    )
                },
            )
        };
        for directive in ["info", "debug", "trace"] {
            assert!(!opens(directive), "{directive}");
        }
        assert!(opens("info,sea_orm=trace"));
        assert!(written!("debug", "sea_orm::schema::builder", DEBUG));
    }
}
