use std::any::TypeId;
use std::str::FromStr;
use std::time::Duration;

use nest_rs_config::{ClientTls, ConfigModule, DurationBounds, Namespaced};
use nest_rs_core::{
    Attempt, Backoff, Budget, Collecting, ContainerBuilder, DynamicModule, Proving, Unproved,
    process_seed, prove,
};
use sea_orm::sqlx::postgres::PgConnectOptions;
use sea_orm::{
    ConnectOptions, Database, DatabaseConnection, DatabaseConnectionType, DbErr, RuntimeErr,
    SqlxError,
};

use crate::SeaOrmConfig;
use crate::config::{CONNECT_TIMEOUT, DEFAULT_CONNECT_TIMEOUT, STATEMENT_TIMEOUT};
use crate::error::ConnectError;

/// Where the substrate's variables live: `<PREFIX>_SEAORM__*`.
const NAMESPACE: &str = <SeaOrmConfig as Namespaced>::NAMESPACE;

/// What every binding says when the pool or its config is missing.
pub(crate) const SUBSTRATE_REMEDY: &str = "import `SeaOrmModule::for_root(None)`, which resolves \
                                           `SeaOrmConfig` and opens the one pool every SeaORM \
                                           binding shares";

/// The SeaORM substrate. Import [`SeaOrmModule::for_root`] once, then the
/// bindings your app needs beside it — they share the pool it opens.
pub struct SeaOrmModule;

impl SeaOrmModule {
    /// `None` ⇒ load [`SeaOrmConfig`] from `<PREFIX>_SEAORM__*`; `Some(cfg)` pins
    /// the base those variables overlay, per field, so the environment still
    /// wins over it.
    pub fn for_root(config: impl Into<Option<SeaOrmConfig>>) -> SeaOrmSetup {
        SeaOrmSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`SeaOrmModule::for_root`].
pub struct SeaOrmSetup {
    pinned: Option<SeaOrmConfig>,
}

impl DynamicModule for SeaOrmSetup {
    fn module() -> TypeId {
        TypeId::of::<SeaOrmModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(
            self.pinned.clone(),
            budgets(BudgetReach::Injected)
                .into_iter()
                .fold(builder, ContainerBuilder::provide_meta),
        );
        builder.provide_factory_after::<DatabaseConnection, (SeaOrmConfig,), _, _>(
            |container| async move {
                let config = ConfigModule::resolved::<SeaOrmConfig>(&container)?;
                connect(config.app_connect_options()?).await
            },
        )
    }
}

/// Who reaches the pool: the code injecting it, or — once
/// [`SeaOrmDatabaseModule`](crate::SeaOrmDatabaseModule) installs `Repo`'s
/// executor around every unit of work — any code at all.
#[derive(Clone, Copy)]
pub(crate) enum BudgetReach {
    Injected,
    Ambient,
}

/// The pool's [`Budget`]s: how long a query waits for a connection, and how
/// long a statement runs before Postgres cancels it.
pub(crate) fn budgets(reach: BudgetReach) -> [Budget; 2] {
    let declare = |resource, bounds: &DurationBounds, read| {
        let setting = format!(
            "{}, or `{}` in code",
            nest_rs_config::var_name(NAMESPACE, bounds.key()),
            bounds.field(),
        );
        match reach {
            BudgetReach::Injected => Budget::of::<DatabaseConnection>(resource, setting, read),
            BudgetReach::Ambient => Budget::ambient::<DatabaseConnection>(resource, setting, read),
        }
    };
    [
        declare("the SeaORM pool", &CONNECT_TIMEOUT, acquire_budget),
        declare("the SeaORM statements", &STATEMENT_TIMEOUT, statement_bound),
    ]
}

/// How long a statement on `db`'s pool runs before Postgres cancels it, read
/// off the options its connections open with; `None` for a connection holding
/// no pool, or one opened without the bound.
fn statement_bound(db: &DatabaseConnection) -> Option<Duration> {
    match db.inner {
        DatabaseConnectionType::SqlxPostgresPoolConnection(_) => statement_timeout_in(
            db.get_postgres_connection_pool()
                .connect_options()
                .get_options()?,
        ),
        _ => None,
    }
}

/// The `statement_timeout` a libpq options string sets — `-c
/// statement_timeout=15000` — read as Postgres reads it: milliseconds unless a
/// unit follows; `None` where it is unset or `0`, which Postgres reads as off.
fn statement_timeout_in(options: &str) -> Option<Duration> {
    let mut tokens = options.split_whitespace();
    let mut value = None;
    while let Some(token) = tokens.next() {
        let setting = match token {
            "-c" => tokens.next().unwrap_or_default(),
            other => other
                .strip_prefix("-c")
                .or_else(|| other.strip_prefix("--"))
                .unwrap_or_default(),
        };
        if let Some((name, set)) = setting.split_once('=')
            && name.replace('-', "_") == "statement_timeout"
        {
            value = Some(set);
        }
    }
    let value = value?;
    let digits = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let amount: u64 = value[..digits].parse().ok()?;
    let unit = match value[digits..].trim() {
        "" | "ms" => Duration::from_millis(1),
        "us" => Duration::from_micros(1),
        "s" => Duration::from_secs(1),
        "min" => Duration::from_secs(60),
        "h" => Duration::from_secs(60 * 60),
        "d" => Duration::from_secs(24 * 60 * 60),
        _ => return None,
    };
    (amount > 0).then(|| unit * u32::try_from(amount).unwrap_or(u32::MAX))
}

/// How long a query waits for a connection from `db`'s pool; `None` for a
/// connection holding no pool.
fn acquire_budget(db: &DatabaseConnection) -> Option<Duration> {
    match db.inner {
        DatabaseConnectionType::SqlxPostgresPoolConnection(_) => Some(
            db.get_postgres_connection_pool()
                .options()
                .get_acquire_timeout(),
        ),
        _ => None,
    }
}

/// Open a standalone connection from `<PREFIX>_SEAORM__*`, resolving the same
/// [`SeaOrmConfig`] the app's [`SeaOrmModule`] uses, for tools outside the DI
/// container (`migrate`, `seed`).
pub async fn connect_from_env() -> anyhow::Result<DatabaseConnection> {
    use nest_rs_config::Config;
    connect(SeaOrmConfig::load()?.connect_options()?).await
}

/// The wait between two attempts to open the pool: the Redis connection's
/// curve, so both come back alike after an outage.
const CONNECT_RETRY: Backoff = Backoff::new(Duration::from_millis(250), Duration::from_secs(2));

/// Open a pool with `options`, which [`SeaOrmConfig::connect_options`] built
/// and checked, proved within its connect budget through [`prove`]: what every
/// attempt would meet — credentials or a database Postgres refuses, TLS that
/// fails — refuses the boot at once; anything else, a name that does not
/// resolve yet included, is retried. The URL may carry credentials, so it is
/// never logged: Postgres is named by its host and port.
///
/// Each attempt is cut at the budget left, and the pool keeps the budget as
/// its acquire timeout: an attempt's own would shorten it for the app's life.
async fn connect(options: ConnectOptions) -> anyhow::Result<DatabaseConnection> {
    let endpoint = endpoint(options.get_url());
    let budget = options
        .get_connect_timeout()
        .unwrap_or(DEFAULT_CONNECT_TIMEOUT);
    tracing::info!(
        target: crate::target::ORM,
        endpoint = %endpoint,
        max_connections = ?options.get_max_connections(),
        "connecting to database"
    );
    let opened = prove(
        budget,
        CONNECT_RETRY,
        process_seed(),
        said(&endpoint, budget),
        |_| {
            let options = options.clone();
            let endpoint = &endpoint;
            async move {
                Database::connect(options)
                    .await
                    .map_err(|error| classify(error, endpoint))
            }
        },
    )
    .await;
    opened.map_err(|ended| unproved(ended, &endpoint, budget).into())
}

/// What the proof of the pool on `endpoint` reports, said on `nest_rs::orm`.
fn said(endpoint: &str, budget: Duration) -> impl FnMut(Proving<'_, DbErr>) + '_ {
    move |proving| match proving {
        Proving::Retrying { attempt, error } => tracing::warn!(
            target: crate::target::ORM,
            endpoint = %endpoint,
            attempt,
            error = %nest_rs_core::error_message(error),
            "database unreachable — retrying within the connect budget",
        ),
        Proving::TimedOut { attempt } => tracing::warn!(
            target: crate::target::ORM,
            endpoint = %endpoint,
            attempt,
            timeout_secs = budget.as_secs_f64(),
            "database connect timed out",
        ),
        Proving::Proved { attempts } if attempts > 1 => tracing::info!(
            target: crate::target::ORM,
            endpoint = %endpoint,
            attempts,
            "database connected after retrying",
        ),
        _ => {}
    }
}

/// What one failed attempt met: a refusal every attempt would repeat, or a
/// failure that may clear — a refused or reset connection, a name that does
/// not resolve yet, a server starting up (`57P03`) or full (`53300`).
fn classify(error: DbErr, endpoint: &str) -> Attempt<DbErr, ConnectError> {
    let DbErr::Conn(RuntimeErr::SqlxError(sqlx)) = &error else {
        return Attempt::Failed(error);
    };
    let url = nest_rs_config::var_name(NAMESPACE, "URL");
    let reason = match &**sqlx {
        SqlxError::Database(server) => match server.code().as_deref() {
            Some(code @ ("28P01" | "28000")) => format!(
                "it does not let in the user {url} names, with the password it gives                  (SQLSTATE {code})"
            ),
            Some("3D000") => {
                format!("it has no database by the name {url} gives (SQLSTATE 3D000)")
            }
            _ => return Attempt::Failed(error),
        },
        SqlxError::Tls(_) | SqlxError::Io(_) if is_tls(sqlx) => format!(
            "TLS failed — name the authority that signed its certificate in {url}'s              `sslrootcert`, and a client certificate in `sslcert` and `sslkey` if Postgres              requires one"
        ),
        SqlxError::Configuration(_) => {
            format!("sqlx refused the options {url} sets — check its query parameters")
        }
        _ => return Attempt::Failed(error),
    };
    Attempt::Refused(ConnectError::Refused {
        endpoint: endpoint.to_owned(),
        reason,
        source: error,
    })
}

/// Whether `sqlx` met TLS that every attempt would fail the same way: TLS it
/// could not set up (`Error::Tls` — an `sslrootcert` it cannot read, a server
/// without TLS), or a handshake rustls refused, which sqlx hands back as an io
/// error holding rustls's own.
fn is_tls(sqlx: &SqlxError) -> bool {
    matches!(sqlx, SqlxError::Tls(_)) || ClientTls::negotiation_failed(sqlx)
}

/// The boot error a proof of the pool on `endpoint` that ended without it
/// ends in.
fn unproved(
    ended: Unproved<DbErr, ConnectError>,
    endpoint: &str,
    budget: Duration,
) -> ConnectError {
    match ended {
        Unproved::Refused(refusal) => refusal,
        Unproved::Spent { attempts, last } => ConnectError::Unreachable {
            endpoint: endpoint.to_owned(),
            budget,
            attempts,
            url: nest_rs_config::var_name(NAMESPACE, "URL"),
            timeout: nest_rs_config::var_name(NAMESPACE, CONNECT_TIMEOUT.key()),
            source: last,
        },
    }
}

/// The host and port `url` dials — or its socket — never the URL, which
/// carries the password.
fn endpoint(url: &str) -> String {
    match PgConnectOptions::from_str(url) {
        Ok(options) => match options.get_socket() {
            Some(socket) => socket.display().to_string(),
            None => format!("{}:{}", options.get_host(), options.get_port()),
        },
        Err(_) => "<a URL sqlx cannot read>".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_statement_bound_is_read_as_postgres_reads_it() {
        for (options, bound) in [
            ("-c statement_timeout=15000", Some(Duration::from_secs(15))),
            (
                "-c geqo=off -c statement_timeout=2s",
                Some(Duration::from_secs(2)),
            ),
            ("--statement-timeout=5min", Some(Duration::from_secs(300))),
            ("-cstatement_timeout=1h", Some(Duration::from_secs(3600))),
            ("-c statement_timeout=0", None),
            ("-c geqo=off", None),
            ("-c statement_timeout=fast", None),
        ] {
            assert_eq!(statement_timeout_in(options), bound, "{options}");
        }
    }
}
