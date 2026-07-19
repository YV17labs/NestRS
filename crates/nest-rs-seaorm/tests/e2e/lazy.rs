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
    let lazy = Arc::new(LazyTransaction::new((*conn).clone()));

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
    let lazy = Arc::new(LazyTransaction::new((*conn).clone()));

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
