//! `DbContext` opens a real transaction around mutating handlers — commits on
//! 2xx/3xx, rolls back on anything else, and surfaces a leaked executor as a
//! loud 500 (silent rollback of a "successful" mutation is data loss).

use std::sync::Arc;
use std::time::Duration;

use nest_rs_interceptors::InterceptorExt;
use nest_rs_seaorm::{DatabaseConfig, DbContext, current_executor};
use poem::endpoint::make;
use poem::http::{Method, StatusCode};
use poem::{Endpoint, IntoResponse, Request, Response, Result};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

fn config() -> Arc<DatabaseConfig> {
    Arc::new(DatabaseConfig::default())
}

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

#[tokio::test]
async fn an_escaped_transaction_fails_an_otherwise_successful_response() {
    let ctx = DbContext::new(crate::harness::connect_arc().await, config());

    let endpoint = make(|_req: Request| async {
        let escaped = current_executor().expect("the handler runs with an ambient executor");
        tokio::spawn(async move {
            let _hold = escaped;
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        StatusCode::OK.into_response()
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a leaked transaction must surface as a 500, never a false 2xx",
    );
}

#[tokio::test]
async fn a_well_behaved_mutating_handler_keeps_its_status() {
    let ctx = DbContext::new(crate::harness::connect_arc().await, config());

    let endpoint = make(|_req: Request| async {
        current_executor().expect("the handler runs with an ambient executor");
        StatusCode::CREATED.into_response()
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn a_mapped_error_2xx_rolls_back_the_handlers_writes() {
    let conn = crate::harness::connect_arc().await;

    // A committed scratch table on the pool, isolated from the request txn.
    conn.execute_unprepared("DROP TABLE IF EXISTS mapped_rollback_probe")
        .await
        .expect("drop any leftover probe table");
    conn.execute_unprepared("CREATE TABLE mapped_rollback_probe (id INT PRIMARY KEY)")
        .await
        .expect("create the probe table");

    let ctx = DbContext::new(conn.clone(), config());

    // A handler that writes inside the request transaction, then hands back a
    // 2xx tagged `MappedError` — exactly what a route-site Filter emits after
    // mapping the handler's `Err`. `DbContext` must roll back regardless of the
    // success status: the mapping shapes the client answer, it does not bless
    // the failed handler's writes.
    let endpoint = make(|_req: Request| async {
        let executor = current_executor().expect("the handler runs with an ambient executor");
        let inserted = executor
            .execute_unprepared("INSERT INTO mapped_rollback_probe (id) VALUES (1)")
            .await
            .expect("the insert runs inside the request transaction");
        assert_eq!(
            inserted.rows_affected(),
            1,
            "the write really lands inside the open transaction",
        );

        let mut resp = StatusCode::OK.into_response();
        resp.extensions_mut().insert(nest_rs_core::MappedError);
        resp
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(
        status,
        StatusCode::OK,
        "the mapped success status is still returned to the client",
    );

    // The pool sees the committed, empty table: the tagged 2xx rolled the insert
    // back rather than committing it behind a success status.
    let remaining: i32 = conn
        .query_one_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            "SELECT COUNT(*)::int AS n FROM mapped_rollback_probe",
        ))
        .await
        .expect("count on the pool")
        .expect("count returns a row")
        .try_get("", "n")
        .expect("read the count");
    assert_eq!(
        remaining, 0,
        "a MappedError-tagged 2xx must roll back the handler's writes",
    );

    conn.execute_unprepared("DROP TABLE IF EXISTS mapped_rollback_probe")
        .await
        .expect("clean up the probe table");
}
