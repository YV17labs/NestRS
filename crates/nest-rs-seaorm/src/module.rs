use std::any::TypeId;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nest_rs_config::{BoundedSetting, ClientTls, ConfigModule, ConfigSetup, Namespaced};
use nest_rs_core::{
    Attempt, Backoff, Budget, Collecting, ContainerBuilder, Module, Proving, Registering,
    SHUTDOWN_SETTLE_TIMEOUT, Unnamed, Unproved, process_seed, prove,
};
use sea_orm::sqlx::Connection as _;
use sea_orm::sqlx::postgres::{PgConnectOptions, PgConnection};
use sea_orm::{
    ConnectOptions, Database, DatabaseConnection, DatabaseConnectionType, DbErr, RuntimeErr,
    SqlxError,
};

use crate::SeaOrmConfig;
use crate::config::{DEFAULT_CONNECT_TIMEOUT, PGSSLROOTCERT, TrustRoot};
use crate::error::ConnectError;
use crate::statements::Statements;

/// Where the substrate's variables live: `<PREFIX>_SEAORM__*`.
const NAMESPACE: &str = <SeaOrmConfig as Namespaced>::NAMESPACE;

nest_rs_core::inventory::submit! {
    nest_rs_core::DependencyRemedy::new(
        TypeId::of::<DatabaseConnection>,
        "Import `nest_rs::seaorm::SeaOrmModule`, which resolves `SeaOrmConfig` and opens the one \
         pool every SeaORM binding shares.",
    )
}

/// The SeaORM substrate: the pool, its budgets, its close and, under the
/// `health` feature, its indicator. Every binding imports it, so an app names
/// it only to pin it, through [`SeaOrmModule::for_root`], or for a pool reached
/// by injection alone, bare, which reads `<PREFIX>_SEAORM__*`. It is imported
/// by type, so two imports open, close and report one pool.
pub struct SeaOrmModule;

impl SeaOrmModule {
    /// Pin `config` as the base `<PREFIX>_SEAORM__*` overlays, per field; the
    /// bare import reads those variables over [`SeaOrmConfig`]'s defaults.
    pub fn for_root(config: SeaOrmConfig) -> SeaOrmSetup {
        ConfigModule::setup(config)
    }
}

/// The configured import [`SeaOrmModule::for_root`] returns.
pub type SeaOrmSetup = ConfigSetup<SeaOrmModule, SeaOrmConfig>;

impl Module for SeaOrmModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature::<Unnamed, SeaOrmConfig>(builder);
        budgets(BudgetReach::Injected)
            .into_iter()
            .fold(builder, ContainerBuilder::provide_meta)
            .provide_factory_after::<DatabaseConnection, (SeaOrmConfig,), _, _>(
                |container| async move {
                    let config = ConfigModule::resolved::<SeaOrmConfig>(&container)?;
                    let mut db = connect(&config, config.app_connect_options()?).await?;
                    Statements::install(&mut db);
                    Ok(db)
                },
            )
            .provide_close::<DatabaseConnection, _, _>(|db| async move { close(&db).await })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        #[cfg(feature = "health")]
        let builder = crate::health::register::<Self>(builder);
        builder
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
    let declare = |resource, timeout: &BoundedSetting<Duration>, read| {
        let setting = timeout.var_or_field(NAMESPACE);
        match reach {
            BudgetReach::Injected => Budget::of::<DatabaseConnection>(resource, setting, read),
            BudgetReach::Ambient => Budget::ambient::<DatabaseConnection>(resource, setting, read),
        }
    };
    [
        declare(
            "the SeaORM pool",
            &SeaOrmConfig::CONNECT_TIMEOUT,
            acquire_budget,
        ),
        declare(
            "the SeaORM statements",
            &SeaOrmConfig::STATEMENT_TIMEOUT,
            statement_bound,
        ),
    ]
}

/// How long a statement on `db`'s pool runs before Postgres cancels it, read
/// off the options its connections open with; `None` for a connection holding
/// no pool, or one opened without the bound.
pub(crate) fn statement_bound(db: &DatabaseConnection) -> Option<Duration> {
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

/// How often the pool's close looks for a connection a settled unit is still
/// handing back.
const RETURN_POLL: Duration = Duration::from_millis(5);

/// Close `db`'s pool once every unit of work settled — the binding's close
/// drains them first: each connection is taken back as it comes, waited for
/// [`SHUTDOWN_SETTLE_TIMEOUT`] at most, and has said goodbye when this
/// returns, a failed goodbye the close's error once every other is said; one
/// still checked out then falls with the process, said at `warn`.
///
/// Not sqlx's close alone, which says goodbye to none until every connection
/// is back, and none for one still out: sqlx 0.9 hands it back from a task
/// begun before the close (`.claude/decisions/pool-closes-at-the-settle.md`).
async fn close(db: &DatabaseConnection) -> anyhow::Result<()> {
    let DatabaseConnectionType::SqlxPostgresPoolConnection(_) = db.inner else {
        return Ok(db.close_by_ref().await?);
    };
    let pool = db.get_postgres_connection_pool();
    let started = tokio::time::Instant::now();
    // A settled unit hands its connection back on a task of sqlx's, after a
    // round trip, and no event says when: each is taken as it lands, before the
    // pool is marked closed, which refuses every acquire.
    let mut idle = Vec::new();
    loop {
        idle.extend(std::iter::from_fn(|| pool.try_acquire()));
        if idle.len() >= pool.size() as usize || started.elapsed() >= SHUTDOWN_SETTLE_TIMEOUT {
            break;
        }
        tokio::time::sleep(RETURN_POLL).await;
    }
    let checked_out = (pool.size() as usize).saturating_sub(idle.len());
    let closing = pool.close();
    // Awaited here, never left to sqlx's tasks: the process may end as soon as
    // the close returns, and a goodbye still queued would fall with it.
    let said = every_goodbye(idle.into_iter().map(|connection| connection.close())).await;
    if checked_out == 0 {
        closing.await;
    } else {
        tracing::warn!(
            target: crate::target::ORM,
            checked_out,
            waited_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "pool closed with connections still checked out: each falls with the process",
        );
    }
    said
}

/// Await each of `goodbyes` in turn, one failing stopping none of the others,
/// as sqlx's own close goes on: the first failure, with how many failed, or
/// none.
async fn every_goodbye<E>(
    goodbyes: impl IntoIterator<Item = impl Future<Output = Result<(), E>>>,
) -> anyhow::Result<()>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let (mut taken, mut failed, mut first) = (0_usize, 0_usize, None);
    for goodbye in goodbyes {
        taken += 1;
        if let Err(error) = goodbye.await {
            failed += 1;
            first.get_or_insert(error);
        }
    }
    first.map_or(Ok(()), |error| {
        Err(anyhow::Error::new(error).context(format!(
            "{failed} of the {taken} connections the pool's close took back failed their \
             goodbye, the first for this reason"
        )))
    })
}

/// Open a standalone connection from `<PREFIX>_SEAORM__*`, resolving the same
/// [`SeaOrmConfig`] the app's [`SeaOrmModule`] uses, for tools outside the DI
/// container (`migrate`, `seed`).
pub async fn connect_from_env() -> anyhow::Result<DatabaseConnection> {
    use nest_rs_config::Config;
    let config = SeaOrmConfig::load()?;
    connect(&config, config.connect_options()?).await
}

/// Open a pool with `options`, which `config` built and checked, proved within
/// its connect budget through [`prove`]: what every attempt would meet —
/// credentials or a database Postgres refuses, TLS that fails — refuses the
/// boot at once; anything else, a refused connection, a server starting up or
/// full and a name that does not resolve yet included, is retried and said.
/// The URL may carry credentials, so it is never logged: Postgres is named by
/// its host and port.
///
/// Each attempt dials one connection before the pool opens: sqlx's pool would
/// retry a refused, starting or full server in silence until its acquire
/// timeout, the whole budget, which it keeps for the app's life.
async fn connect(
    config: &SeaOrmConfig,
    options: ConnectOptions,
) -> anyhow::Result<DatabaseConnection> {
    let probe = config.probe_options()?;
    let root = config.trust_root();
    let endpoint = endpoint(&probe);
    let budget = options
        .get_connect_timeout()
        .unwrap_or(DEFAULT_CONNECT_TIMEOUT);
    tracing::info!(
        target: crate::target::ORM,
        endpoint = %endpoint,
        max_connections = options.get_max_connections(),
        "connecting to database"
    );
    prove(
        budget,
        Backoff::RECONNECT,
        process_seed(),
        said(&endpoint, budget),
        |_| {
            let (probe, options, endpoint) = (&probe, options.clone(), &endpoint);
            async move {
                let met = |error| {
                    classify(
                        DbErr::Conn(RuntimeErr::SqlxError(Arc::new(error))),
                        endpoint,
                        root,
                    )
                };
                PgConnection::connect_with(probe)
                    .await
                    .map_err(met)?
                    .close()
                    .await
                    .map_err(met)?;
                Database::connect(options)
                    .await
                    .map_err(|error| classify(error, endpoint, root))
            }
        },
    )
    .await
    .map_err(|ended| unproved(ended, &endpoint, budget).into())
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
            timeout_ms = budget.as_millis(),
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
/// not resolve yet, a server starting up (`57P03`) or full (`53300`). A TLS
/// refusal says where the authority is named, `root` telling where it was read.
fn classify(error: DbErr, endpoint: &str, root: TrustRoot) -> Attempt<DbErr, ConnectError> {
    let DbErr::Conn(RuntimeErr::SqlxError(sqlx)) = &error else {
        return Attempt::Failed(error);
    };
    let url = SeaOrmConfig::URL.var(NAMESPACE);
    let reason = match &**sqlx {
        SqlxError::Database(server) => match server.code().as_deref() {
            Some(code @ ("28P01" | "28000")) => format!(
                "it does not let in the user {url} names, with the password it gives \
                 (SQLSTATE {code})"
            ),
            Some("3D000") => {
                format!("it has no database by the name {url} gives (SQLSTATE 3D000)")
            }
            _ => return Attempt::Failed(error),
        },
        SqlxError::Tls(_) | SqlxError::Io(_) if is_tls(sqlx) => {
            let authority = match root {
                TrustRoot::Variable => format!(
                    "libpq's `{PGSSLROOTCERT}`, which this process sets, or in {url}'s \
                     `sslrootcert`"
                ),
                TrustRoot::System | TrustRoot::Url => format!("{url}'s `sslrootcert`"),
            };
            format!(
                "TLS failed — name the authority that signed its certificate in {authority}, \
                 and a client certificate in `sslcert` and `sslkey` if Postgres requires one"
            )
        }
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
/// could not set up (`Error::Tls` — a certificate file that does not parse, a
/// server without TLS), or a handshake rustls refused, which sqlx hands back as
/// an io error holding rustls's own. A file it cannot read is an io error too,
/// which `connect_options` refuses first.
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
            url: SeaOrmConfig::URL.var(NAMESPACE),
            timeout: SeaOrmConfig::CONNECT_TIMEOUT.var(NAMESPACE),
            source: last,
        },
    }
}

/// The host and port `options` dial — or their socket — never the URL, which
/// carries the password.
pub(crate) fn endpoint(options: &PgConnectOptions) -> String {
    match socket(options) {
        Some(socket) => socket.display().to_string(),
        None => format!("{}:{}", options.get_host(), options.get_port()),
    }
}

/// The socket `options` dial, as sqlx reads it: the one a URL's `host=` names,
/// or a host starting with `/`, where `PGHOST` and sqlx's default put theirs.
pub(crate) fn socket(options: &PgConnectOptions) -> Option<&Path> {
    options.get_socket().map(PathBuf::as_path).or_else(|| {
        let host = options.get_host();
        host.starts_with('/').then(|| Path::new(host))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_that_expires_is_said_in_milliseconds() {
        let logs = nest_rs_testing::LogCapture::install();

        said("postgres://db", Duration::from_millis(300))(Proving::TimedOut { attempt: 2 });

        let line = logs.expect_one(crate::target::ORM, "database connect timed out");
        assert_eq!(line.level, "warn");
        assert_eq!(line.field("attempt").as_deref(), Some("2"));
        assert_eq!(line.field("timeout_ms").as_deref(), Some("300"));
    }

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

    /// The tools' path opens as the app's does, so both say a plaintext URL,
    /// once, before anything answers.
    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn opening_the_pool_says_its_plaintext_url_once() {
        use nest_rs_config::{Config as _, Environment};
        figment::Jail::expect_with(|jail| {
            jail.set_env(Environment::var_name(), "production");
            jail.set_env(
                nest_rs_config::var_name("seaorm", "URL"),
                "postgres://app:s3cret@192.0.2.1/app?sslmode=disable",
            );
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            let answer_never = Duration::from_millis(1);
            let logs = nest_rs_testing::LogCapture::install();
            let tools = runtime
                .block_on(async { tokio::time::timeout(answer_never, connect_from_env()).await });
            assert!(tools.is_err(), "nothing answers there");
            logs.expect_one(crate::target::ORM, crate::config::PLAINTEXT_OPEN);
            drop(logs);

            let config = SeaOrmConfig::load().expect("the variable reads");
            let logs = nest_rs_testing::LogCapture::install();
            let options = config.app_connect_options().expect("the app's pool");
            let app = runtime.block_on(async {
                tokio::time::timeout(answer_never, connect(&config, options)).await
            });
            assert!(app.is_err(), "nothing answers there");
            logs.expect_one(crate::target::ORM, crate::config::PLAINTEXT_OPEN);
            Ok(())
        });
    }

    /// A socket reads as its path whichever option names it: sqlx dials a host
    /// starting with `/` as one, which is where `PGHOST` and its default put it.
    #[test]
    fn the_endpoint_reads_a_socket_as_its_path_however_it_is_named() {
        let options = PgConnectOptions::new_without_pgpass().port(5433);
        for socket in [
            options.clone().host("/run/postgresql"),
            options.clone().socket("/run/postgresql"),
        ] {
            assert_eq!(endpoint(&socket), "/run/postgresql");
        }
        assert_eq!(endpoint(&options.host("db")), "db:5433");
    }

    #[tokio::test]
    async fn a_goodbye_that_fails_stops_none_of_the_others_and_the_first_is_the_closes_error() {
        let said = std::sync::Mutex::new(Vec::new());
        let goodbye = |n: u8, refused: bool| {
            let said = &said;
            async move {
                said.lock().unwrap_or_else(|e| e.into_inner()).push(n);
                (!refused).ok_or_else(|| std::io::Error::other(format!("goodbye {n} refused")))
            }
        };

        let failed = every_goodbye([goodbye(1, true), goodbye(2, false), goodbye(3, true)])
            .await
            .expect_err("two goodbyes failed");

        assert_eq!(*said.lock().unwrap_or_else(|e| e.into_inner()), [1, 2, 3]);
        assert_eq!(
            nest_rs_core::error_message(&*failed),
            "2 of the 3 connections the pool's close took back failed their goodbye, the first \
             for this reason: goodbye 1 refused"
        );
        every_goodbye([goodbye(4, false)])
            .await
            .expect("every goodbye said");
    }
}
