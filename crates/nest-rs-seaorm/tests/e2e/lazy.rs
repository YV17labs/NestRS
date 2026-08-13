//! The lazy request executor: `BEGIN` is deferred to the first data-layer
//! touch, so a guard-denied mutating request opens **zero** transactions,
//! while a handler that writes still commits through the lazily opened one.

use std::sync::Arc;

use nest_rs_interceptors::InterceptorExt;
use nest_rs_seaorm::{
    DatabaseConfig, DbContext, Executor, LazyTransaction, current_executor, with_request_executor,
};
use poem::endpoint::make;
use poem::http::{Method, StatusCode};
use poem::{Endpoint, IntoResponse, Request, Response, Result};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

fn mutating_request() -> Request {
    Request::builder()
        .method(Method::POST)
        .uri("/".parse().unwrap())
        .finish()
}

fn status_of(result: Result<Response>) -> StatusCode {
    match result {
        Ok(resp) => resp.status(),
        Err(err) => err.into_response().status(),
    }
}

// The counting seam itself: no data-layer touch ⇒ the cell stays empty ⇒ no
// `BEGIN` was ever issued against Postgres.
#[tokio::test]
async fn no_data_layer_touch_opens_no_transaction() {
    let conn = crate::harness::connect_arc().await;
    let lazy = Arc::new(LazyTransaction::new((*conn).clone(), "test"));

    with_request_executor(Executor::Lazy(lazy.clone()), async {
        // Simulates a guard denial: the request scope exists, nothing queries.
    })
    .await;

    assert!(
        !lazy.is_opened(),
        "a request that never touches the data layer must not open a transaction",
    );
}

#[tokio::test]
async fn first_query_opens_the_transaction_once() {
    let conn = crate::harness::connect_arc().await;
    let lazy = Arc::new(LazyTransaction::new((*conn).clone(), "test"));

    with_request_executor(Executor::Lazy(lazy.clone()), async {
        let executor = current_executor().expect("ambient executor installed");
        executor
            .execute_unprepared("SELECT 1")
            .await
            .expect("the first query opens the transaction and runs");
        executor
            .execute_unprepared("SELECT 1")
            .await
            .expect("subsequent queries reuse the same transaction");
    })
    .await;

    assert!(lazy.is_opened(), "a data-layer touch opens the transaction");
    let txn = Arc::try_unwrap(lazy)
        .ok()
        .and_then(LazyTransaction::into_opened)
        .expect("exactly one transaction was opened");
    let txn = Arc::try_unwrap(txn).expect("no lingering clone after the scope ended");
    txn.rollback()
        .await
        .expect("rollback the probe transaction");
}

// End-to-end through `DbContext`: a denied mutating request (403 before any
// query) flows through unchanged — the finalizer finds no transaction to
// commit or roll back.
#[tokio::test]
async fn a_denied_mutating_request_passes_through_with_no_transaction() {
    let ctx = DbContext::new(
        crate::harness::connect_arc().await,
        Arc::new(DatabaseConfig::default()),
    );

    let endpoint = make(|_req: Request| async { StatusCode::FORBIDDEN.into_response() });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// End-to-end through `DbContext`: a handler that writes commits through the
// lazily opened transaction — visible from the pool afterwards.
#[tokio::test]
async fn a_writing_handler_commits_through_the_lazy_transaction() {
    let conn = crate::harness::connect_arc().await;
    conn.execute_unprepared("DROP TABLE IF EXISTS lazy_commit_probe")
        .await
        .expect("drop any leftover probe table");
    conn.execute_unprepared("CREATE TABLE lazy_commit_probe (id INT PRIMARY KEY)")
        .await
        .expect("create the probe table");

    let ctx = DbContext::new(conn.clone(), Arc::new(DatabaseConfig::default()));
    let endpoint = make(|_req: Request| async {
        let executor = current_executor().expect("ambient executor installed");
        executor
            .execute_unprepared("INSERT INTO lazy_commit_probe (id) VALUES (1)")
            .await
            .expect("the insert opens and rides the lazy transaction");
        StatusCode::CREATED.into_response()
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(status, StatusCode::CREATED);

    let count: i32 = conn
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT COUNT(*)::int AS n FROM lazy_commit_probe",
        ))
        .await
        .expect("count on the pool")
        .expect("count returns a row")
        .try_get("", "n")
        .expect("read the count");
    assert_eq!(count, 1, "the lazily opened transaction must commit");

    conn.execute_unprepared("DROP TABLE IF EXISTS lazy_commit_probe")
        .await
        .expect("clean up the probe table");
}

// ---------------------------------------------------------------------------
// What a **connection** fault costs an attempt. Both cases below are ones the
// framework knows wrote nothing — no `COMMIT` was ever issued — so both are
// retryable, and both were classified `deterministic` and dead-lettered before
// `is_transient_failure` existed. The in-doubt commit at the end is the
// deliberate exception, and it is what keeps the classification honest.

/// The pool never hands out a connection, so `BEGIN` cannot even be issued.
/// Nothing reached the database; an outage is exactly what a retry budget is
/// for.
#[tokio::test]
async fn a_pool_that_never_hands_out_a_connection_is_retryable() {
    let pool = crate::harness::starved_pool().await;

    // Hold the only connection for the duration of the attempt.
    let hog = sea_orm::TransactionTrait::begin(&pool)
        .await
        .expect("hold the only connection");

    let lazy = Arc::new(LazyTransaction::new(pool.clone(), "test"));
    let reported_success = with_request_executor(Executor::Lazy(lazy.clone()), async {
        let executor = current_executor().expect("ambient executor installed");
        // The shape `Poisoned` exists for: the job swallows the `DbErr` and
        // reports success.
        let _ = executor.execute_unprepared("SELECT 1").await;
        true
    })
    .await;

    match lazy.finalize(reported_success).await {
        nest_rs_seaorm::FinalizeOutcome::Poisoned { retryable } => assert!(
            retryable,
            "a pool acquire timeout wrote nothing, so the attempt is worth repeating",
        ),
        other => panic!("expected a poisoned boundary, got {other:?}"),
    }
    hog.rollback().await.expect("release the held connection");
}

/// The table the terminated attempt writes to, so "nothing landed" is a row
/// count rather than an assumption.
const TERMINATED_PROBE_TABLE: &str = "lazy_terminated_probe";

/// The server closes the session mid-attempt (`57P01`). Postgres rolls the
/// transaction back on close, so again nothing landed.
#[tokio::test]
async fn a_backend_terminated_mid_attempt_is_retryable() {
    let pool = crate::harness::connect().await;
    let killer = crate::harness::connect().await;
    crate::harness::setup_shared_table(
        &pool,
        TERMINATED_PROBE_TABLE,
        &format!("CREATE TABLE IF NOT EXISTS {TERMINATED_PROBE_TABLE} (id INT PRIMARY KEY);"),
    )
    .await;

    let lazy = Arc::new(LazyTransaction::new(pool.clone(), "test"));
    let reported_success = with_request_executor(Executor::Lazy(lazy.clone()), async {
        let executor = current_executor().expect("ambient executor installed");
        executor
            .execute_unprepared(&format!(
                "INSERT INTO {TERMINATED_PROBE_TABLE} (id) VALUES (1) ON CONFLICT DO NOTHING"
            ))
            .await
            .expect("the first statement opens the transaction and writes");
        let pid = crate::harness::backend_pid(&executor).await;
        crate::harness::terminate_backend(&killer, pid).await;
        let _ = executor.execute_unprepared("SELECT 1").await;
        true
    })
    .await;

    match lazy.finalize(reported_success).await {
        nest_rs_seaorm::FinalizeOutcome::Poisoned { retryable } => assert!(
            retryable,
            "a connection the server closed rolled the attempt back, so a replay \
             cannot write twice",
        ),
        other => panic!("expected a poisoned boundary, got {other:?}"),
    }

    // The claim the classification rests on, asserted rather than assumed: the
    // attempt's write is not durable. `retryable: true` is only safe because of
    // this row count.
    let surviving = pool
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("SELECT count(*) FROM {TERMINATED_PROBE_TABLE}"),
        ))
        .await
        .expect("count the attempt's writes")
        .expect("one row")
        .try_get_by_index::<i64>(0)
        .expect("a count");
    assert_eq!(
        surviving, 0,
        "the terminated attempt left nothing behind, which is what makes \
         replaying it safe",
    );
}

// ---------------------------------------------------------------------------
// An **abandoned** boundary: the future holding the executor is dropped before
// anything settles it. That is the framework's own shutdown path — a queue
// worker's `shutdown_timeout` elapsing makes apalis drop the job future where
// it stands — and if that is mid-statement, the transaction stays open until
// the abandoned statement drains server-side. Nothing can cancel it from here,
// so what the framework owes is the event.

#[tokio::test]
async fn a_boundary_abandoned_mid_statement_says_so() {
    let logs = nest_rs_testing::LogCapture::install();

    let conn = crate::harness::connect().await;
    let lazy = Arc::new(LazyTransaction::new(conn.clone(), "worker"));
    {
        let attempt = nest_rs_seaorm::with_job_executor(Executor::Lazy(Arc::clone(&lazy)), async {
            let executor = current_executor().expect("ambient executor installed");
            executor
                .execute_unprepared("SELECT 1")
                .await
                .expect("the first statement opens the transaction");
            // In flight when the attempt is dropped, which is the case that
            // costs something: the rollback cannot go out until it drains.
            let _ = executor.execute_unprepared("SELECT pg_sleep(1)").await;
            true
        });
        // What a shutdown timeout does: drop the job future where it stands.
        let elapsed = tokio::time::timeout(std::time::Duration::from_millis(200), attempt).await;
        assert!(elapsed.is_err(), "the attempt is abandoned, not finished");
    }
    assert!(lazy.is_opened(), "the abandoned attempt had opened one");
    drop(lazy);

    let event = logs.expect_one(
        "nest_rs::orm",
        "transaction abandoned without settling; its locks are held until the abandoned \
         statement drains",
    );
    assert_eq!(
        event.field("transport").as_deref(),
        Some("worker"),
        "the event names the edge that abandoned it",
    );
    assert_eq!(
        event.field("outcome").as_deref(),
        Some("abandoned"),
        "and the outcome an operator greps on",
    );
}

/// The deliberate exception, and the one that keeps the widened classification
/// honest: the connection is lost with the transaction **clean**, so nothing
/// poisoned it and `finalize` reaches the `COMMIT` — which then fails without
/// saying whether it landed. That one is **not** replayed. Widening
/// `CommitError::is_retryable_conflict` to match `is_transient_failure` would
/// turn "may have written once" into "wrote twice"; this is what would catch it.
#[tokio::test]
async fn a_commit_whose_outcome_is_unknown_stays_deterministic() {
    let pool = crate::harness::connect().await;
    let killer = crate::harness::connect().await;

    let lazy = Arc::new(LazyTransaction::new(pool.clone(), "test"));
    let pid = with_request_executor(Executor::Lazy(Arc::clone(&lazy)), async {
        let executor = current_executor().expect("ambient executor installed");
        executor
            .execute_unprepared("SELECT 1")
            .await
            .expect("the first statement opens the transaction");
        crate::harness::backend_pid(&executor).await
    })
    .await;

    // Killed *after* the last statement returned, so no statement failed and the
    // boundary is clean: the failure lands on the `COMMIT` itself.
    crate::harness::terminate_backend(&killer, pid).await;

    match lazy.finalize(true).await {
        nest_rs_seaorm::FinalizeOutcome::CommitFailed(err) => assert!(
            !err.is_retryable_conflict(),
            "a commit that may have landed is never replayed, whatever the \
             statement-level classification says about the same error: {err}",
        ),
        other => panic!("expected the commit itself to fail, got {other:?}"),
    }
}
