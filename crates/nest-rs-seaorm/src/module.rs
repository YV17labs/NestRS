//! [`SeaOrmModule`] — the substrate seam. `SeaOrmModule::for_root(None)` resolves
//! [`SeaOrmConfig`] and opens the one `sea_orm::DatabaseConnection` every SeaORM
//! binding shares: `SeaOrmDatabaseModule` (the `Executor` port and the request
//! layers), `SeaOrmHealthModule` (the health indicator), the worker context.
//! Each binding is imported bare beside it and reads the pool from the
//! container.
//!
//! The crate-root `module.rs` a driver is allowed exactly once: a module *of
//! SeaORM* — the crate's own subject — and not one binding wearing the crate's
//! name.

use std::any::TypeId;
use std::time::Duration;

use nest_rs_config::{ConfigModule, DurationBounds, Namespaced};
use nest_rs_core::{Budget, Collecting, ContainerBuilder, DynamicModule};
use sea_orm::{ConnectOptions, Database, DatabaseConnection, DatabaseConnectionType};

use crate::SeaOrmConfig;
use crate::config::{CONNECT_TIMEOUT, STATEMENT_TIMEOUT};

/// Where the substrate's variables live: `<PREFIX>_SEAORM__*`.
const NAMESPACE: &str = <SeaOrmConfig as Namespaced>::NAMESPACE;

/// What every binding says when the pool or its config is missing — one
/// sentence, every site, so a reader who forgot the substrate is told the same
/// thing by whichever binding noticed first.
pub(crate) const SUBSTRATE_REMEDY: &str = "import `SeaOrmModule::for_root(None)`, which resolves \
                                           `SeaOrmConfig` and opens the one pool every SeaORM \
                                           binding shares";

/// The SeaORM substrate. Import [`SeaOrmModule::for_root`] once, then the
/// bindings your app needs beside it — they share the pool it opens.
pub struct SeaOrmModule;

impl SeaOrmModule {
    /// `None` ⇒ load [`SeaOrmConfig`] from `<PREFIX>_SEAORM__*`; `Some(cfg)` pins
    /// the base those variables overlay, per field.
    ///
    /// A pin is not a test hatch: the deployment's real environment still wins
    /// over it. A test that must not read the ambient environment seeds the
    /// value instead — `App::builder().provide(cfg)` short-circuits the factory.
    pub fn for_root(config: impl Into<Option<SeaOrmConfig>>) -> SeaOrmSetup {
        SeaOrmSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`SeaOrmModule::for_root`]. Resolves the
/// config and queues the async pool factory, so every binding's factory —
/// wherever it falls in `imports = [..]` — finds the pool already built.
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
            connect(&config, config.app_connect_options()).await
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
/// [`SeaOrmConfig`] the app's [`SeaOrmModule`] uses. The single connector for
/// tools outside the DI container (`migrate`, `seed`) — a new config knob
/// reaches them without editing each binary.
pub async fn connect_from_env() -> anyhow::Result<DatabaseConnection> {
    use nest_rs_config::Config;
    let config = SeaOrmConfig::load()?;
    connect(&config, config.connect_options()).await
}

/// Open `config`'s pool with `options`. The URL may carry credentials, so it is
/// never logged.
async fn connect(
    config: &SeaOrmConfig,
    options: ConnectOptions,
) -> anyhow::Result<DatabaseConnection> {
    if config.url.is_empty() {
        anyhow::bail!(
            "{} must be set",
            nest_rs_config::spellings(NAMESPACE, "URL")
        );
    }
    // A config seeded on the builder skips `from_env`, and with it the range
    // the variable is held to: checked again where the budget is spent, before
    // sqlx adds it to a clock.
    if let Some(secs) = config.connect_timeout_secs {
        CONNECT_TIMEOUT.check(
            NAMESPACE,
            CONNECT_TIMEOUT.field(),
            Duration::from_secs(secs),
        )?;
    }
    if let Some(secs) = config.statement_timeout_secs {
        STATEMENT_TIMEOUT.check(
            NAMESPACE,
            STATEMENT_TIMEOUT.field(),
            Duration::from_secs(secs),
        )?;
    }
    tracing::info!(
        target: crate::TARGET,
        max_connections = ?config.max_connections,
        "connecting to database"
    );
    Ok(Database::connect(options).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// config-4r2, the seeded path: a config handed to the builder skips
    /// `from_env`, and the top of the range reached sqlx and panicked the boot.
    /// The check where the budget is spent refuses it naming the variable, before
    /// anything is dialled.
    #[tokio::test]
    async fn a_seeded_budget_past_the_ceiling_is_refused_before_sqlx_sees_it() {
        let config = SeaOrmConfig {
            url: "postgres://nobody@127.0.0.1:1/none".to_owned(),
            connect_timeout_secs: Some(u64::MAX),
            ..SeaOrmConfig::default()
        };
        let refused = connect(&config, config.app_connect_options())
            .await
            .expect_err("refused rather than handed to sqlx")
            .to_string();
        assert!(
            refused.contains(&nest_rs_config::var_name("seaorm", "CONNECT_TIMEOUT_SECS"))
                && refused.contains("above the 3600s it must be at most"),
            "{refused}"
        );
    }

    /// The statement bound a seeded config carries is held to its range where
    /// the pool is opened, as the acquire budget is.
    #[tokio::test]
    async fn a_seeded_statement_bound_past_the_ceiling_is_refused_before_the_pool_opens() {
        let config = SeaOrmConfig {
            url: "postgres://nobody@127.0.0.1:1/none".to_owned(),
            statement_timeout_secs: Some(60 * 60 + 1),
            ..SeaOrmConfig::default()
        };
        let refused = connect(&config, config.app_connect_options())
            .await
            .expect_err("refused rather than handed to Postgres")
            .to_string();
        assert!(
            refused.contains(&nest_rs_config::var_name(
                "seaorm",
                "STATEMENT_TIMEOUT_SECS"
            )) && refused.contains("above the 3600s it must be at most"),
            "{refused}"
        );
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
}
