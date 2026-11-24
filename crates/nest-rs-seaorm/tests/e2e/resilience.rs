//! A resilience policy's attempts over SeaORM: a call declared idempotent runs
//! each attempt in a savepoint of its unit's transaction, so a failed attempt
//! leaves no write for the next to repeat — rolled back once it failed, was cut
//! at its timeout mid-statement or met a statement the database refused, what
//! its unit wrote before it standing — whatever retries its policy has, none
//! included; and nothing is sent for a call that never queries, nor on the
//! pool, which holds no transaction.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use nest_rs_core::{injectable, instance, module};
use nest_rs_database::__private::with_request_executor;
use nest_rs_resilience::{
    ResilienceConfig, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
};
use nest_rs_seaorm::__private::{
    FinalizeOutcome, LazyTransaction, SeaOrmExecutor, current_executor,
};
use nest_rs_testing::HeadlessApp;
use sea_orm::prelude::Uuid;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};

#[instance]
struct Ledger;

/// A dependency whose policy has its retries off.
#[instance]
struct Archive;

#[derive(Debug, thiserror::Error)]
enum Failure {
    #[error("the ledger's statement failed")]
    Db(#[from] DbErr),
    #[error("the ledger refused the entry")]
    Refused,
}

/// A service writing its ledger under the policy, as an app's does: through
/// the ambient executor, each attempt of a method declared idempotent.
#[injectable]
struct LedgerService {
    #[inject]
    ledger: Arc<ResiliencePolicy<Ledger>>,
    #[inject]
    archive: Arc<ResiliencePolicy<Archive>>,
    attempts: AtomicU32,
}

impl LedgerService {
    /// One row of `entry`, the first attempt refused once it wrote its own.
    #[resilient(ledger, idempotent)]
    async fn post(&self, entry: Uuid) -> Result<(), ResilienceError<Failure>> {
        write(entry).await?;
        if self.attempts.fetch_add(1, Ordering::Relaxed) == 0 {
            return Err(Failure::Refused);
        }
        Ok(())
    }

    /// One row of `entry`, the first attempt cut at its timeout while a
    /// statement it sent after writing is still running.
    #[resilient(ledger, idempotent, timeout = "200ms")]
    async fn post_slowly(&self, entry: Uuid) -> Result<(), ResilienceError<Failure>> {
        write(entry).await?;
        if self.attempts.fetch_add(1, Ordering::Relaxed) == 0 {
            outlast_the_timeout().await?;
        }
        Ok(())
    }

    /// One row of `entry`, every attempt refused once it wrote its own.
    #[resilient(archive, idempotent)]
    async fn archive(&self, entry: Uuid) -> Result<(), ResilienceError<Failure>> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        write(entry).await?;
        Err(Failure::Refused)
    }
}

#[module(
    imports = [
        ResilienceModule::for_root((Ledger, ResilienceConfig {
            retries: Some(2),
            backoff_first: Duration::from_millis(1),
            backoff_ceiling: Duration::from_millis(2),
            breaker_window: None,
            ..ResilienceConfig::default()
        })),
        ResilienceModule::for_root((Archive, ResilienceConfig {
            retries: None,
            breaker_window: None,
            ..ResilienceConfig::default()
        })),
    ],
    providers = [LedgerService],
)]
struct LedgerModule;

async fn db() -> DatabaseConnection {
    let conn = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &conn,
        "resilience_probe",
        "CREATE TABLE IF NOT EXISTS resilience_probe (id UUID PRIMARY KEY, entry UUID NOT NULL);",
    )
    .await;
    conn
}

async fn booted() -> HeadlessApp {
    HeadlessApp::builder()
        .module::<LedgerModule>()
        .build_headless()
        .await
        .expect("the policy boots")
}

async fn policy() -> (HeadlessApp, Arc<ResiliencePolicy<Ledger>>) {
    let app = booted().await;
    let policy = app
        .container()
        .get::<ResiliencePolicy<Ledger>>()
        .expect("the module built the policy");
    (app, policy)
}

async fn service() -> (HeadlessApp, Arc<LedgerService>) {
    let app = booted().await;
    let service = app
        .container()
        .get::<LedgerService>()
        .expect("the module lists the service");
    (app, service)
}

/// One row of `entry`, written on the ambient executor.
async fn write(entry: Uuid) -> Result<(), DbErr> {
    write_as(Uuid::now_v7(), entry).await
}

/// The row `id` of `entry`, written on the ambient executor.
async fn write_as(id: Uuid, entry: Uuid) -> Result<(), DbErr> {
    let executor = current_executor().ok_or_else(|| DbErr::Custom("no executor".into()))?;
    executor
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO resilience_probe (id, entry) VALUES ($1, $2)",
            [id.into(), entry.into()],
        ))
        .await
        .map(|_| ())
}

/// A statement on the ambient executor still running at the 200 ms an attempt
/// of [`LedgerService::post_slowly`] is given up at: the test lasts as long,
/// since the rollback waits for it.
async fn outlast_the_timeout() -> Result<(), DbErr> {
    let executor = current_executor().ok_or_else(|| DbErr::Custom("no executor".into()))?;
    executor
        .execute_unprepared("SELECT pg_sleep(1)")
        .await
        .map(|_| ())
}

fn unit(conn: &DatabaseConnection) -> Arc<LazyTransaction> {
    Arc::new(LazyTransaction::new(conn.clone(), "test"))
}

async fn committed(unit: Arc<LazyTransaction>) {
    let outcome = unit.finalize(true).await;
    assert!(
        matches!(outcome, FinalizeOutcome::Committed),
        "the unit commits what it wrote and what its run kept, got {outcome:?}"
    );
}

/// How many rows of `entry` the table holds.
async fn written(conn: &DatabaseConnection, entry: Uuid) -> i64 {
    conn.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT COUNT(*) AS rows FROM resilience_probe WHERE entry = $1",
        [entry.into()],
    ))
    .await
    .expect("the probe reads")
    .expect("a count answers one row")
    .try_get("", "rows")
    .expect("the count")
}

#[tokio::test]
async fn an_attempt_that_failed_leaves_no_write_for_the_next_to_repeat() {
    let conn = db().await;
    let (_app, policy) = policy().await;
    let entry = Uuid::now_v7();
    let attempts = AtomicU32::new(0);

    let lazy = Arc::new(LazyTransaction::new(conn.clone(), "test"));
    let ran = with_request_executor(
        SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(),
        policy.run_idempotent(|_| async {
            write(entry).await?;
            if attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                return Err(Failure::Refused);
            }
            Ok(())
        }),
    )
    .await;
    assert!(ran.is_ok(), "the second attempt answers: {ran:?}");
    assert_eq!(attempts.load(Ordering::Relaxed), 2);
    let outcome = lazy.finalize(true).await;
    assert!(
        matches!(outcome, FinalizeOutcome::Committed),
        "the unit commits what its last attempt wrote, got {outcome:?}"
    );

    assert_eq!(
        written(&conn, entry).await,
        1,
        "the failed attempt's row was rolled back before the next attempt wrote its own"
    );
}

/// A decorated method's attempts each run in a savepoint: the refused one is
/// rolled back alone, and what its unit wrote before the call stands.
#[tokio::test]
async fn a_decorated_methods_failed_attempt_is_rolled_back_alone_and_its_units_earlier_write_stands()
 {
    let conn = db().await;
    let (_app, service) = service().await;
    let (before, entry) = (Uuid::now_v7(), Uuid::now_v7());

    let lazy = unit(&conn);
    let ran = with_request_executor(SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(), async {
        write(before)
            .await
            .expect("the unit writes before the call");
        service.post(entry).await
    })
    .await;
    assert!(ran.is_ok(), "the second attempt answers: {ran:?}");
    assert_eq!(service.attempts.load(Ordering::Relaxed), 2);
    committed(lazy).await;

    assert_eq!(written(&conn, before).await, 1);
    assert_eq!(written(&conn, entry).await, 1);
}

/// An attempt given up at its timeout leaves a statement running: its future
/// is dropped there, and its rollback is awaited behind that statement, so the
/// unit stays able to commit what the retry writes.
#[tokio::test]
async fn an_attempt_cut_at_its_timeout_mid_statement_is_rolled_back_and_its_retry_commits() {
    let logs = nest_rs_testing::LogCapture::install();
    let conn = db().await;
    let (_app, service) = service().await;
    let entry = Uuid::now_v7();

    let lazy = unit(&conn);
    let ran = with_request_executor(
        SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(),
        service.post_slowly(entry),
    )
    .await;
    assert!(ran.is_ok(), "the second attempt answers: {ran:?}");
    assert_eq!(service.attempts.load(Ordering::Relaxed), 2);
    committed(lazy).await;

    assert_eq!(
        written(&conn, entry).await,
        1,
        "the row the cut attempt wrote was rolled back, the retry's committed"
    );
    logs.expect_none(
        nest_rs_seaorm::target::ORM,
        "atomic block abandoned before it settled; its unit cannot commit",
    );
}

/// A statement Postgres refuses aborts its transaction up to the last
/// savepoint: the attempt's, so the retry writes on and the unit commits.
#[tokio::test]
async fn a_statement_refused_inside_an_attempt_leaves_its_retry_free_to_write_and_commit() {
    let conn = db().await;
    let (_app, policy) = policy().await;
    let (taken, entry) = (Uuid::now_v7(), Uuid::now_v7());
    let attempts = AtomicU32::new(0);

    let lazy = unit(&conn);
    let ran = with_request_executor(SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(), async {
        write_as(taken, entry).await.expect("the unit writes first");
        policy
            .run_idempotent(|_| async {
                let id = match attempts.fetch_add(1, Ordering::Relaxed) {
                    0 => taken,
                    _ => Uuid::now_v7(),
                };
                Ok::<_, Failure>(write_as(id, entry).await?)
            })
            .await
    })
    .await;
    assert!(ran.is_ok(), "the second attempt answers: {ran:?}");
    assert_eq!(attempts.load(Ordering::Relaxed), 2);
    committed(lazy).await;

    assert_eq!(
        written(&conn, entry).await,
        2,
        "the unit's row and the retry's"
    );
}

/// A run ending on its last attempt's error leaves its unit as it found it:
/// able to commit, what it wrote before the run standing, nothing of an attempt.
#[tokio::test]
async fn a_run_whose_last_attempt_fails_leaves_its_unit_its_earlier_write_and_nothing_of_an_attempt()
 {
    let conn = db().await;
    let (_app, policy) = policy().await;
    let (before, entry) = (Uuid::now_v7(), Uuid::now_v7());
    let attempts = AtomicU32::new(0);

    let lazy = unit(&conn);
    let ran = with_request_executor(SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(), async {
        write(before)
            .await
            .expect("the unit writes before the call");
        policy
            .run_idempotent(|_| async {
                attempts.fetch_add(1, Ordering::Relaxed);
                write(entry).await?;
                Err::<(), _>(Failure::Refused)
            })
            .await
    })
    .await;
    assert!(
        matches!(ran, Err(ResilienceError::Failed(Failure::Refused))),
        "{ran:?}"
    );
    assert_eq!(attempts.load(Ordering::Relaxed), 3);
    committed(lazy).await;

    assert_eq!(written(&conn, before).await, 1);
    assert_eq!(
        written(&conn, entry).await,
        0,
        "the last attempt is rolled back as the others are"
    );
}

/// What a failed call leaves is read off its declaration, never off the
/// retries its policy has: under a policy with none, an idempotent method is
/// attempted once and that attempt rolled back, so its unit commits what it
/// wrote before the call and nothing of it.
#[tokio::test]
async fn an_idempotent_call_under_a_policy_with_no_retries_leaves_no_write_once_it_failed() {
    let conn = db().await;
    let (_app, service) = service().await;
    let (before, entry) = (Uuid::now_v7(), Uuid::now_v7());

    let lazy = unit(&conn);
    let ran = with_request_executor(SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(), async {
        write(before)
            .await
            .expect("the unit writes before the call");
        service.archive(entry).await
    })
    .await;
    assert!(
        matches!(ran, Err(ResilienceError::Failed(Failure::Refused))),
        "{ran:?}"
    );
    assert_eq!(
        service.attempts.load(Ordering::Relaxed),
        1,
        "not attempted again"
    );
    committed(lazy).await;

    assert_eq!(written(&conn, before).await, 1);
    assert_eq!(
        written(&conn, entry).await,
        0,
        "the one attempt is rolled back, as the last is with retries on"
    );
}

/// A savepoint is sent with its attempt's first statement: a call that never
/// queries, attempted twice, opens no transaction at all.
#[tokio::test]
async fn a_call_that_never_queries_sends_nothing() {
    let conn = db().await;
    let (_app, policy) = policy().await;
    let attempts = AtomicU32::new(0);

    let lazy = unit(&conn);
    let ran = with_request_executor(
        SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(),
        policy.run_idempotent(|_| async {
            if attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                return Err(Failure::Refused);
            }
            Ok(())
        }),
    )
    .await;
    assert!(ran.is_ok(), "{ran:?}");
    assert_eq!(attempts.load(Ordering::Relaxed), 2);

    assert!(!lazy.is_opened(), "no BEGIN, no SAVEPOINT");
    let outcome = lazy.finalize(true).await;
    assert!(
        matches!(outcome, FinalizeOutcome::NoTransaction),
        "{outcome:?}"
    );
}

/// Nothing can be awaited as a run is dropped: its attempt's savepoint goes as
/// the port says of a block dropped before it settled, and its unit commits
/// nothing.
#[tokio::test]
async fn a_run_dropped_mid_attempt_leaves_its_unit_unable_to_commit() {
    let logs = nest_rs_testing::LogCapture::install();
    let conn = db().await;
    let (_app, policy) = policy().await;
    let (before, entry) = (Uuid::now_v7(), Uuid::now_v7());
    let wrote = tokio::sync::Notify::new();

    let lazy = unit(&conn);
    with_request_executor(SeaOrmExecutor::Lazy(Arc::clone(&lazy)).into(), async {
        write(before)
            .await
            .expect("the unit writes before the call");
        let run = policy.run_idempotent(|_| async {
            write(entry).await?;
            wrote.notify_one();
            std::future::pending::<Result<(), Failure>>().await
        });
        tokio::select! {
            ran = run => panic!("the attempt never answers: {ran:?}"),
            () = wrote.notified() => {}
        }
    })
    .await;

    let outcome = lazy.finalize(true).await;
    assert!(
        matches!(outcome, FinalizeOutcome::Poisoned { retryable: false }),
        "{outcome:?}"
    );
    assert_eq!(written(&conn, before).await, 0);
    assert_eq!(written(&conn, entry).await, 0);
    let line = logs.expect_one(
        nest_rs_seaorm::target::ORM,
        "atomic block abandoned before it settled; its unit cannot commit",
    );
    assert_eq!(line.level, "warn");
}

/// The pool holds no transaction to open a savepoint in, and an attempt opens
/// none of its own: each statement commits as it is sent, a failed attempt's
/// included, as with no policy.
#[tokio::test]
async fn on_the_pool_an_attempt_opens_no_transaction_and_its_writes_commit_as_they_are_sent() {
    let conn = db().await;
    let (_app, policy) = policy().await;
    let entry = Uuid::now_v7();
    let attempts = AtomicU32::new(0);

    let ran = with_request_executor(
        SeaOrmExecutor::Pool(conn.clone()).into(),
        policy.run_idempotent(|_| async {
            let on_the_pool = matches!(current_executor(), Some(SeaOrmExecutor::Pool(_)));
            write(entry).await?;
            if attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                return Err(Failure::Refused);
            }
            Ok(on_the_pool)
        }),
    )
    .await;
    assert!(matches!(ran, Ok(true)), "{ran:?}");

    assert_eq!(
        written(&conn, entry).await,
        2,
        "a write on the pool is its statement's own commit: no attempt undoes it"
    );
}
