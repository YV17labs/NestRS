//! Covers `src/module.rs`: the pool declares its budget, so the boot holds it
//! under the net of a guard whose strategy injects the pool; what Postgres
//! refuses on every attempt refuses the boot at once; libpq's `PGSSLROOTCERT`
//! is the root its TLS verifies to; and the pool closes once every unit
//! settled, whatever a connection still checked out waits on.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nest_rs_authn::{AUTHENTICATE_TIMEOUT, AuthError, AuthnGuard, Strategy};
use nest_rs_core::{App, BudgetPastNetError, hooks, injectable, module};
use nest_rs_http::Request;
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmModule};
use nest_rs_testing::EphemeralDatabase;
use sea_orm::{ConnectionTrait, DatabaseConnection};

use crate::harness::NoMigrations;
use crate::system_store::stand_in_system_store;

/// A strategy that injects the pool, as one looking an identity up does.
#[injectable]
struct PoolStrategy {
    #[inject]
    #[expect(
        dead_code,
        reason = "injected only so the strategy's code reaches the pool"
    )]
    db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Strategy for PoolStrategy {
    type Principal = crate::Anyone;

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        Err(AuthError::MissingCredentials)
    }
}

type PoolGuard = AuthnGuard<PoolStrategy>;

#[module(
    imports = [SeaOrmModule],
    providers = [PoolStrategy, PoolGuard],
)]
struct GuardedPoolModule;

/// A strategy that injects nothing.
#[injectable]
struct LocalStrategy;

#[async_trait]
impl Strategy for LocalStrategy {
    type Principal = crate::Anyone;

    async fn authenticate(&self, _req: &mut Request) -> Result<Self::Principal, AuthError> {
        Err(AuthError::MissingCredentials)
    }
}

type LocalGuard = AuthnGuard<LocalStrategy>;

#[module(
    imports = [SeaOrmModule],
    providers = [LocalStrategy, LocalGuard],
)]
struct UnboundPoolModule;

/// Seeded rather than pinned, so the suite's own variables cannot move it.
pub(crate) fn config(budget: Option<Duration>) -> SeaOrmConfig {
    let defaults = SeaOrmConfig::default();
    SeaOrmConfig {
        url: crate::harness::url().into(),
        connect_timeout: budget.unwrap_or(defaults.connect_timeout),
        ..defaults
    }
}

#[tokio::test]
async fn a_pool_budget_at_the_net_of_a_guard_injecting_it_fails_the_boot() {
    let Err(refused) = App::builder()
        .provide(config(Some(AUTHENTICATE_TIMEOUT)))
        .module::<GuardedPoolModule>()
        .build()
        .await
    else {
        panic!("a pool waiting as long as the guard must not boot");
    };
    let refused = refused
        .downcast::<BudgetPastNetError>()
        .unwrap_or_else(|other| panic!("not a budget refusal: {other:#}"));
    assert_eq!(
        (refused.resource, refused.port, refused.budget, refused.net),
        (
            "the SeaORM pool",
            "the authentication guard",
            AUTHENTICATE_TIMEOUT,
            AUTHENTICATE_TIMEOUT
        )
    );
    assert!(
        refused
            .setting
            .contains(&nest_rs_config::var_name("seaorm", "CONNECT_TIMEOUT_SECS")),
        "{refused}"
    );
}

#[tokio::test]
async fn the_default_pool_boots_beside_the_authentication_guard() {
    App::builder()
        .provide(config(None))
        .module::<GuardedPoolModule>()
        .build()
        .await
        .expect("the default budget sits under every net");
}

#[tokio::test]
async fn a_pool_no_executor_installs_is_not_held_under_a_strategy_injecting_nothing() {
    App::builder()
        .provide(config(Some(AUTHENTICATE_TIMEOUT)))
        .module::<UnboundPoolModule>()
        .build()
        .await
        .expect("without the database binding, `Repo` reaches no pool from the strategy");
}

/// A pool whose statements Postgres cancels past `statement_timeout`.
pub(crate) fn bounding_statements(bound: Duration) -> SeaOrmConfig {
    SeaOrmConfig {
        statement_timeout: bound,
        ..config(None)
    }
}

#[module(imports = [SeaOrmModule])]
struct PoolModule;

/// The dev container's authority, which each environment this suite runs in
/// installs in the system's store: the dev container's start
/// (`.devcontainer/scripts/post-start.sh`) and CI's `dev-services` action.
const DEV_AUTHORITY: &str = "/usr/local/share/ca-certificates/nestrs-development.crt";

/// A process whose store lacks the dev authority is refused by the suite's
/// Postgres, naming where an authority is named, and logs in once libpq's
/// `PGSSLROOTCERT` names it: the variable is the root of verification, as
/// libpq reads it. The store is the suite's stand-in, read once per process.
#[test]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn a_store_lacking_the_dev_authority_logs_in_on_pgsslrootcert_alone() {
    let elsewhere = nest_rs_testing::TestAuthority::new();
    figment::Jail::expect_with(|jail| {
        stand_in_system_store(jail, elsewhere.pem());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let boot = || {
            runtime.block_on(
                App::builder()
                    .provide(config(None))
                    .module::<PoolModule>()
                    .build(),
            )
        };

        let Err(refused) = boot() else {
            panic!("the store trusts the dev authority after all");
        };
        let refused = format!("{refused:#}");
        assert!(
            refused.contains("invalid peer certificate") && refused.contains("`sslrootcert`"),
            "{refused}"
        );

        jail.set_env("PGSSLROOTCERT", DEV_AUTHORITY);
        let app = boot().unwrap_or_else(|refused| panic!("the variable names it: {refused:#}"));
        runtime.block_on(async move {
            let db = app
                .container()
                .get::<DatabaseConnection>()
                .expect("the pool");
            db.execute_unprepared("SELECT 1")
                .await
                .expect("a statement runs over the connection the variable verified");
            app.close().await;
        });
        Ok(())
    });
}

/// Postgres cancels a statement running past the bound and says why, so a lock
/// held elsewhere or a plan gone wrong ends as the database's own error.
#[tokio::test]
async fn a_statement_past_its_bound_is_cancelled_by_postgres() {
    let app = App::builder()
        .provide(bounding_statements(Duration::from_secs(1)))
        .module::<PoolModule>()
        .build()
        .await
        .expect("boots");
    let db = app
        .container()
        .get::<DatabaseConnection>()
        .expect("the pool");
    // Run whole, the sleep succeeds: only Postgres cancelling it fails it, so
    // the failure is the bound's, however long the machine took to say so.
    let cancelled = db
        .execute_unprepared("SELECT pg_sleep(3)")
        .await
        .expect_err("a statement past its bound is cancelled");
    assert!(
        cancelled.to_string().contains("statement timeout"),
        "{cancelled}"
    );
}

#[tokio::test]
async fn a_statement_bound_at_the_net_of_a_guard_injecting_the_pool_fails_the_boot() {
    let Err(refused) = App::builder()
        .provide(bounding_statements(AUTHENTICATE_TIMEOUT))
        .module::<GuardedPoolModule>()
        .build()
        .await
    else {
        panic!("statements running as long as the guard must not boot");
    };
    let refused = refused
        .downcast::<BudgetPastNetError>()
        .unwrap_or_else(|other| panic!("not a budget refusal: {other:#}"));
    assert_eq!(
        (refused.resource, refused.budget),
        ("the SeaORM statements", AUTHENTICATE_TIMEOUT)
    );
    assert!(
        refused.setting.contains(&nest_rs_config::var_name(
            "seaorm",
            "STATEMENT_TIMEOUT_SECS"
        )),
        "{refused}"
    );
}

/// `url` with its userinfo's password replaced by `password`.
fn with_password(url: &str, password: &str) -> String {
    let (scheme, rest) = url.split_once("://").expect("a URL with a scheme");
    let (userinfo, host) = rest
        .split_once('@')
        .expect("the suite's URL names its user");
    let user = userinfo.split_once(':').map_or(userinfo, |(user, _)| user);
    format!("{scheme}://{user}:{password}@{host}")
}

/// `url` naming the database `name`.
fn with_database(url: &str, name: &str) -> String {
    let (base, query) = url
        .split_once('?')
        .map_or((url, None), |(base, q)| (base, Some(q)));
    let (scheme, rest) = base.split_once("://").expect("a URL with a scheme");
    let host = rest.split_once('/').map_or(rest, |(host, _)| host);
    let query = query.map(|q| format!("?{q}")).unwrap_or_default();
    format!("{scheme}://{host}/{name}{query}")
}

/// The connect budget [`refused_at`] boots under: a refusal retried ends when
/// it does, so one said before it was said at once, however loaded the machine.
const ONLY_A_RETRY_SPENDS: Duration = Duration::from_secs(60);

/// Boots a pool on `url` under [`ONLY_A_RETRY_SPENDS`], and returns its refusal
/// with how long it took.
async fn refused_at(url: String) -> (anyhow::Error, Duration) {
    let began = std::time::Instant::now();
    let Err(refused) = App::builder()
        .provide(SeaOrmConfig {
            url: url.into(),
            connect_timeout: ONLY_A_RETRY_SPENDS,
            ..Default::default()
        })
        .module::<PoolModule>()
        .build()
        .await
    else {
        panic!("a pool Postgres refuses must not boot");
    };
    (refused, began.elapsed())
}

/// Refused credentials fail every attempt the same way: the boot refuses at
/// once, naming the variable and the SQLSTATE, never the password nor the
/// server's own sentence, and retries nothing.
#[tokio::test]
async fn a_password_postgres_refuses_fails_the_boot_at_once() {
    let logs = nest_rs_testing::LogCapture::install();
    let (refused, took) =
        refused_at(with_password(&crate::harness::url(), "n0t-the-password")).await;

    assert!(took < ONLY_A_RETRY_SPENDS, "at once, took {took:?}");
    let said = nest_rs_core::error_message(&*refused);
    assert!(
        said.contains("SQLSTATE 28P01")
            && said.contains(&nest_rs_config::var_name("seaorm", "URL")),
        "{said}"
    );
    assert!(
        !said.contains("n0t-the-password") && !said.contains("password authentication failed"),
        "neither the password nor the server's sentence: {said}"
    );
    assert!(!said.contains("  "), "one sentence, unpadded: {said}");
    let printed = nest_rs_testing::printed(&*refused);
    assert!(
        !printed.contains("n0t-the-password"),
        "what a live server answers holds no password, by any reader: {printed}"
    );
    logs.expect_none(
        nest_rs_seaorm::target::ORM,
        "database unreachable — retrying within the connect budget",
    );
}

/// A database the server does not have fails every attempt the same way too.
#[tokio::test]
async fn a_database_postgres_does_not_have_fails_the_boot_at_once() {
    let logs = nest_rs_testing::LogCapture::install();
    let (refused, took) = refused_at(with_database(
        &crate::harness::url(),
        "nestrs_boot_proof_absent",
    ))
    .await;

    assert!(took < ONLY_A_RETRY_SPENDS, "at once, took {took:?}");
    let said = nest_rs_core::error_message(&*refused);
    assert!(
        said.contains("SQLSTATE 3D000")
            && said.contains(&nest_rs_config::var_name("seaorm", "URL")),
        "{said}"
    );
    assert!(!said.contains("nestrs_boot_proof_absent"), "{said}");
    assert!(!said.contains("  "), "one sentence, unpadded: {said}");
    logs.expect_none(
        nest_rs_seaorm::target::ORM,
        "database unreachable — retrying within the connect budget",
    );
}

/// `url` naming its connections `name` in `pg_stat_activity`.
fn with_application_name(url: &str, name: &str) -> String {
    let joiner = if url.contains('?') { '&' } else { '?' };
    format!("{url}{joiner}application_name={name}")
}

/// How many of the server's connections carry `name`, asked from `observer`.
async fn connections_named(observer: &DatabaseConnection, name: &str) -> i64 {
    let row = observer
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT count(*) AS open FROM pg_stat_activity WHERE application_name = $1",
            [name.into()],
        ))
        .await
        .expect("pg_stat_activity answers")
        .expect("a count is one row");
    row.try_get::<i64>("", "open").expect("the count")
}

/// How long a test waits for Postgres to end a session told goodbye: a guard
/// against a hang alone, since the server runs a goodbye whenever it next
/// schedules its session, however late.
const GOODBYES: Duration = Duration::from_secs(60);

/// How many sessions to `database` ended on a socket cut rather than a goodbye,
/// as Postgres counts them for that database alone.
async fn sessions_cut(database: &EphemeralDatabase) -> i64 {
    database
        .connection()
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT sessions_abandoned AS cut FROM pg_stat_database \
             WHERE datname = current_database()",
        ))
        .await
        .expect("pg_stat_database answers")
        .expect("one row for the database")
        .try_get::<i64>("", "cut")
        .expect("the count")
}

/// The pool says goodbye to Postgres when the app ends — every connection
/// closed by the app, never left for the server to notice a dropped socket.
#[tokio::test]
async fn closing_the_app_closes_every_connection_its_pool_opened() {
    let database = EphemeralDatabase::create::<NoMigrations>()
        .await
        .expect("a database of its own, whose sessions are the app's");
    let name = format!("nestrs-close-{}", std::process::id());
    let app = App::builder()
        .provide(SeaOrmConfig {
            url: with_application_name(database.url(), &name).into(),
            ..config(None)
        })
        .module::<PoolModule>()
        .build()
        .await
        .expect("boots");
    let db = app
        .container()
        .get::<DatabaseConnection>()
        .expect("the pool");
    db.execute_unprepared("SELECT 1")
        .await
        .expect("the pool answers");
    let observer = crate::harness::connect().await;
    assert!(
        connections_named(&observer, &name).await >= 1,
        "the pool's connections are open"
    );

    app.close().await;

    assert!(
        db.get_postgres_connection_pool().is_closed(),
        "the app closed its pool"
    );
    nest_rs_testing::wait_for(GOODBYES, || async {
        connections_named(&observer, &name).await == 0
    })
    .await;
    assert_eq!(
        sessions_cut(&database).await,
        0,
        "a connection was dropped rather than told goodbye",
    );
}

/// A request's unit, as the HTTP edge declares it.
const REQUEST: nest_rs_core::operation_log::Unit = nest_rs_core::__private::declare_unit(
    "nest-rs-http",
    "nest_rs::http",
    nest_rs_core::operation_log::Kind::Server,
    "http.request",
);

/// A shutdown hook that needs a connection while the way down runs.
#[injectable]
#[derive(Default)]
struct ReadsAtShutdown;

#[hooks]
impl ReadsAtShutdown {
    #[on_application_shutdown]
    async fn read(&self) -> anyhow::Result<()> {
        nest_rs_seaorm::__private::current_executor()
            .ok_or_else(|| anyhow::anyhow!("the hook runs outside its unit"))?
            .execute_unprepared("SELECT 1")
            .await?;
        Ok(())
    }
}

#[module(
    imports = [nest_rs_seaorm::SeaOrmDatabaseModule],
    providers = [ReadsAtShutdown],
)]
struct ClosingModule;

/// How long the cut unit's statement runs on: past any stall the test meets,
/// so its connection is still out whenever the close returns; the test's
/// database ends it, dropped with the test.
const OUTLASTS_THE_CLOSE: &str = "SELECT pg_sleep(60)";

/// How many of the server's connections named `name` still run
/// [`OUTLASTS_THE_CLOSE`], asked from `observer`.
async fn statements_outlasting_the_close(observer: &DatabaseConnection, name: &str) -> i64 {
    let row = observer
        .query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT count(*) AS running FROM pg_stat_activity \
             WHERE application_name = $1 AND state = 'active' AND query = $2",
            [name.into(), OUTLASTS_THE_CLOSE.into()],
        ))
        .await
        .expect("pg_stat_activity answers")
        .expect("a count is one row");
    row.try_get::<i64>("", "running").expect("the count")
}

/// A pool unit cut while Postgres still runs its statement leaves its
/// connection checked out until the statement ends; the pool closes anyway once
/// every hook's unit ended, never waiting it out to the hooks' budget, and
/// every connection it took back says goodbye.
#[tokio::test]
async fn the_pool_closes_promptly_while_a_cut_unit_still_holds_a_connection() {
    let logs = nest_rs_testing::LogCapture::install();
    let database = EphemeralDatabase::create::<NoMigrations>()
        .await
        .expect("a database of its own, whose sessions are the app's");
    let name = format!("nestrs-cut-close-{}", std::process::id());
    let app = App::builder()
        .provide(SeaOrmConfig {
            url: with_application_name(database.url(), &name).into(),
            ..config(None)
        })
        .module::<ClosingModule>()
        .build()
        .await
        .expect("boots");
    app.init().await.expect("the init phases drain");
    let binding = app
        .container()
        .get_dyn::<dyn nest_rs_database::UnitOfWork>()
        .expect("the binding");
    let observer = crate::harness::connect().await;
    let unit = nest_rs_database::run_unit(
        Some(&binding),
        nest_rs_database::UnitSpec::new(
            REQUEST,
            nest_rs_database::ExecutorScope::Request,
            nest_rs_database::TransactionPolicy::Pool,
        ),
        async {
            nest_rs_seaorm::__private::current_executor()
                .expect("the unit's pool")
                .execute_unprepared(OUTLASTS_THE_CLOSE)
                .await
                .map(drop)
        },
        Result::is_ok,
        |_| Ok(()),
    );
    let cut = tokio::select! {
        ran = unit => Some(ran),
        () = nest_rs_testing::wait_for(Duration::from_secs(60), || async {
            statements_outlasting_the_close(&observer, &name).await == 1
        }) => None,
    };
    assert!(
        cut.is_none(),
        "the unit is cut while Postgres still runs its statement"
    );

    app.close().await;

    assert_eq!(
        statements_outlasting_the_close(&observer, &name).await,
        1,
        "the close returned while the cut unit still held its connection: it waited on none",
    );
    nest_rs_testing::wait_for(GOODBYES, || async {
        connections_named(&observer, &name).await == 1
    })
    .await;
    assert_eq!(
        sessions_cut(&database).await,
        0,
        "a connection the close took back was dropped rather than told goodbye",
    );
    logs.expect_none(
        nest_rs_core::target::LIFECYCLE,
        "resource close abandoned: the shutdown hooks' budget was spent while it waited, and \
         the closes after it still start",
    );
    let left = logs.expect_one(
        nest_rs_seaorm::target::ORM,
        "pool closed with connections still checked out: each falls with the process",
    );
    assert_eq!(left.level, "warn");
    assert_eq!(left.field("checked_out").as_deref(), Some("1"));
    let read = logs
        .find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_core::unit::HOOK.name(),
        )
        .into_iter()
        .find(|line| line.field("method").as_deref() == Some("read"))
        .expect("the shutdown hook's line");
    assert_eq!(
        read.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::OK),
        "the hook got a connection while the cut one was still out",
    );
}
