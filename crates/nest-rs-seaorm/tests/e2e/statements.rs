//! Covers `src/statements.rs` against Postgres: once something records client
//! operations, every statement the module's pool sends is handed over by its
//! summary, under the database and the server the URL names — those sea-orm
//! times, and the `BEGIN`, `COMMIT`, `ROLLBACK`, savepoint settles and
//! unprepared text the driver times itself, each once — and the pool is
//! observed under its name while the app holds it.

use std::sync::{Arc, Mutex};

use nest_rs_core::client_operation::{ClientOperation, ClientValue};
use nest_rs_core::{App, module};
use nest_rs_database::__private::with_request_executor;
use nest_rs_database::atomic;
use nest_rs_seaorm::__private::{
    FinalizeOutcome, LazyTransaction, SeaOrmExecutor, current_executor,
};
use nest_rs_seaorm::{RepoError, SeaOrmModule};
use sea_orm::sqlx::postgres::PgConnectOptions;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};

/// One operation the seeded recorder was handed: its name, the database it
/// named, and its class if it failed.
type Recorded = (String, Option<String>, Option<String>);

static RECORDED: Mutex<Vec<Recorded>> = Mutex::new(Vec::new());

/// The `db.response.status_code` of each operation recorded, in order.
static STATUS_CODES: Mutex<Vec<Option<String>>> = Mutex::new(Vec::new());

fn capture(operation: &ClientOperation<'_>) {
    STATUS_CODES
        .lock()
        .expect("the status codes")
        .push(
            operation
                .attributes()
                .iter()
                .find_map(|(key, value)| match (*key, value) {
                    ("db.response.status_code", ClientValue::Text(code)) => {
                        Some((*code).to_owned())
                    }
                    _ => None,
                }),
        );
    let namespace = operation
        .attributes()
        .iter()
        .find_map(|(key, value)| match (*key, value) {
            ("db.namespace", ClientValue::Text(namespace)) => Some((*namespace).to_owned()),
            _ => None,
        });
    RECORDED.lock().expect("the recorded operations").push((
        operation.name().to_owned(),
        namespace,
        operation.error_type().map(str::to_owned),
    ));
}

/// Record every client operation of this process from now on, forgetting any
/// recorded so far.
pub(crate) fn record_operations() {
    nest_rs_core::__private::set_client_recorder(capture);
    RECORDED.lock().expect("the recorded operations").clear();
    STATUS_CODES.lock().expect("the status codes").clear();
}

/// The name of each operation recorded since [`record_operations`].
pub(crate) fn recorded() -> Vec<String> {
    RECORDED
        .lock()
        .expect("the recorded operations")
        .iter()
        .map(|(name, ..)| name.clone())
        .collect()
}

#[module(imports = [SeaOrmModule])]
struct PoolModule;

/// The module's pool, opened once something records client operations.
async fn pool() -> (App, Arc<DatabaseConnection>) {
    record_operations();
    let app = App::builder()
        .provide(crate::module::config(None))
        .module::<PoolModule>()
        .build()
        .await
        .expect("boots");
    let db = app
        .container()
        .get::<DatabaseConnection>()
        .expect("the pool");
    record_operations();
    (app, db)
}

fn url_options() -> PgConnectOptions {
    crate::harness::url()
        .parse::<PgConnectOptions>()
        .expect("a Postgres URL")
}

#[tokio::test]
async fn every_statement_the_pool_sends_is_recorded_by_its_summary() {
    let (_app, db) = pool().await;

    let statement = |sql: &str| Statement::from_string(DbBackend::Postgres, sql);
    db.query_one_raw(statement("SELECT 1")).await.expect("runs");
    db.execute_raw(statement("select current_database()"))
        .await
        .expect("runs");

    let database = url_options().get_database().map(str::to_owned);
    let recorded = RECORDED.lock().expect("the recorded operations").clone();
    assert_eq!(
        recorded,
        vec![
            ("SELECT".to_owned(), database.clone(), None),
            ("SELECT".to_owned(), database, None),
        ],
    );
}

/// Run `body` in a unit's lazy transaction on `db`, settled on `success`.
async fn in_a_unit<F>(db: &DatabaseConnection, success: bool, body: F) -> FinalizeOutcome
where
    F: Future<Output = ()>,
{
    let lazy = Arc::new(LazyTransaction::new(db.clone(), "test"));
    with_request_executor(SeaOrmExecutor::Lazy(lazy.clone()).into(), body).await;
    lazy.finalize(success).await
}

async fn select(sql: &str) {
    let executor = current_executor().expect("ambient executor installed");
    executor.execute_unprepared(sql).await.expect("runs");
}

#[tokio::test]
async fn a_unit_that_commits_is_recorded_from_its_begin_to_its_commit_once_each() {
    let (_app, db) = pool().await;

    let outcome = in_a_unit(&db, true, async {
        select("SELECT 1").await;
        let executor = current_executor().expect("ambient executor installed");
        executor
            .query_one_raw(Statement::from_string(DbBackend::Postgres, "SELECT 2"))
            .await
            .expect("runs");
    })
    .await;

    assert!(matches!(outcome, FinalizeOutcome::Committed), "{outcome:?}");
    assert_eq!(recorded(), ["BEGIN", "SELECT", "SELECT", "COMMIT"]);
}

#[tokio::test]
async fn a_unit_that_fails_is_recorded_to_its_rollback() {
    let (_app, db) = pool().await;

    let outcome = in_a_unit(&db, false, select("SELECT 1")).await;

    assert!(
        matches!(outcome, FinalizeOutcome::RolledBack),
        "{outcome:?}"
    );
    assert_eq!(recorded(), ["BEGIN", "SELECT", "ROLLBACK"]);
}

/// A statement the server refused is recorded failed with the SQLSTATE it
/// answered, as `error.type` and `db.response.status_code`, never `_OTHER`.
#[tokio::test]
async fn a_statement_the_server_refuses_is_recorded_with_its_sqlstate() {
    let (_app, db) = pool().await;

    let outcome = in_a_unit(&db, false, async {
        let executor = current_executor().expect("ambient executor installed");
        executor
            .execute_unprepared("SELECT 1/0")
            .await
            .expect_err("Postgres refuses a division by zero");
    })
    .await;

    assert!(
        matches!(outcome, FinalizeOutcome::RolledBack),
        "{outcome:?}"
    );
    let recorded = RECORDED.lock().expect("the recorded operations").clone();
    let refused = recorded
        .iter()
        .position(|(name, ..)| name == "SELECT")
        .expect("the refused SELECT was recorded");
    assert_eq!(recorded[refused].2.as_deref(), Some("22012"));
    assert_eq!(
        STATUS_CODES.lock().expect("the status codes")[refused].as_deref(),
        Some("22012"),
    );
}

#[tokio::test]
async fn an_atomic_block_is_recorded_from_its_savepoint_to_its_release_or_its_rollback() {
    let (_app, db) = pool().await;

    let outcome = in_a_unit(&db, true, async {
        atomic(async {
            select("SELECT 1").await;
            Ok::<_, RepoError>(())
        })
        .await
        .expect("the block keeps its work");
        let refused = atomic(async {
            select("SELECT 2").await;
            Err::<(), _>(RepoError::NotFound)
        })
        .await;
        assert!(refused.is_err());
    })
    .await;

    assert!(matches!(outcome, FinalizeOutcome::Committed), "{outcome:?}");
    assert_eq!(
        recorded(),
        [
            "BEGIN",
            "SAVEPOINT",
            "SELECT",
            "RELEASE",
            "SAVEPOINT",
            "SELECT",
            "ROLLBACK",
            "COMMIT",
        ],
    );
}

#[tokio::test]
async fn the_pool_is_observed_under_its_server_and_database_while_the_app_holds_it() {
    let (app, db) = pool().await;
    let options = url_options();
    let name = format!(
        "{}:{}/{}",
        options.get_host(),
        options.get_port(),
        options
            .get_database()
            .expect("the suite's URL names its database"),
    );
    let observed = || {
        nest_rs_core::__private::observed_pools()
            .into_iter()
            .find(|pool| pool.name() == name)
    };

    let pool = observed().expect("the module's pool is observed");
    db.execute_unprepared("SELECT 1").await.expect("runs");
    let connections = pool.connections().expect("the pool is open");
    assert_eq!(
        connections.max(),
        u64::from(crate::module::config(None).max_connections),
    );
    assert!(
        connections.idle() + connections.used() >= 1,
        "{connections:?}"
    );

    drop((pool, db));
    app.close().await;
    assert!(
        observed().is_none_or(|pool| pool.connections().is_none()),
        "a closed pool reports no connection",
    );
}

/// What sea-orm times on its own leaves out the text sent through
/// `execute_unprepared` and a transaction's control, which is why the driver
/// times what it sends itself (`.claude/upstream/sea-orm.md`). Fails the day
/// sea-orm times them.
#[tokio::test]
async fn upstream_sea_orm_still_leaves_transaction_control_and_unprepared_text_untimed() {
    let (_app, db) = pool().await;

    db.execute_unprepared("SELECT 1").await.expect("runs");
    let txn = db.begin().await.expect("begins");
    txn.commit().await.expect("commits");

    assert!(
        recorded().is_empty(),
        "sea-orm now times {:?}: drop `Statements::sent` where it does, and close \
         .claude/upstream/sea-orm.md",
        recorded(),
    );
}
