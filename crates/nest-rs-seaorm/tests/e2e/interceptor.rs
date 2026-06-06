//! `DbContext` opens a real transaction around mutating handlers — commits on
//! 2xx/3xx, rolls back on anything else, and surfaces a leaked executor as a
//! loud 500 (silent rollback of a "successful" mutation is data loss).

use std::sync::Arc;
use std::time::Duration;

use nest_rs_seaorm::{DatabaseConfig, DbContext, current_executor};
use nest_rs_middleware::EndpointExt;
use poem::endpoint::make;
use poem::http::{Method, StatusCode};
use poem::{Endpoint, IntoResponse, Request, Response, Result};
use sea_orm::Database;

async fn db() -> Arc<sea_orm::DatabaseConnection> {
    let url = std::env::var("NESTRS_DATABASE__URL")
        .expect("NESTRS_DATABASE__URL must point at a reachable Postgres for this test");
    Arc::new(Database::connect(&url).await.expect("connect to Postgres"))
}

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
    let ctx = DbContext::new(db().await, config());

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
    let ctx = DbContext::new(db().await, config());

    let endpoint = make(|_req: Request| async {
        current_executor().expect("the handler runs with an ambient executor");
        StatusCode::CREATED.into_response()
    });

    let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
    assert_eq!(status, StatusCode::CREATED);
}
