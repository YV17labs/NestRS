use std::any::TypeId;
use std::time::Duration;

use nest_rs_config::{ConfigModule, DurationBounds, Namespaced};
use nest_rs_core::{Budget, Collecting, ContainerBuilder, DynamicModule};
use sea_orm::{ConnectOptions, Database, DatabaseConnection, DatabaseConnectionType};

use crate::SeaOrmConfig;
use crate::config::{CONNECT_TIMEOUT, STATEMENT_TIMEOUT};

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
        builder.provide_factory::<DatabaseConnection, _, _>(|container| async move {
            #[expect(
                clippy::expect_used,
                reason = "provide_feature queued the config's factory in this module's collect"
            )]
            let config = container
                .get::<SeaOrmConfig>()
                .expect("SeaOrmConfig is resolved by ConfigModule::provide_feature");
            connect(config.app_connect_options()?).await
        })
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

/// Open a pool with `options`, which
/// [`SeaOrmConfig::connect_options`] built and checked. The URL may carry
/// credentials, so it is never logged.
async fn connect(options: ConnectOptions) -> anyhow::Result<DatabaseConnection> {
    tracing::info!(
        target: crate::target::ORM,
        max_connections = ?options.get_max_connections(),
        "connecting to database"
    );
    Ok(Database::connect(options).await?)
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
