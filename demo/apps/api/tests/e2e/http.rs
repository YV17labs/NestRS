use api::ApiModule;
use nest_rs_authn::JwtConfig;
use nest_rs_http::HttpTransport;
use nest_rs_testing::{EphemeralDatabase, TestApp};
use poem::http::{StatusCode, header};
use serde_json::json;

use super::harness::*;

#[tokio::test]
async fn responses_are_gzip_compressed_when_the_client_accepts_it() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let app = TestApp::builder()
        .module::<ApiModule>()
        .http(HttpTransport::new().compression(true))
        .with_test_telemetry()
        .provide_arc(db.connection())
        .provide(JwtConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            ..Default::default()
        })
        .build()
        .await
        .expect("ApiModule boots on a compression-enabled transport");
    let _db = db;
    let bearer = format!("Bearer {}", login().await);

    let compressed = app
        .http()
        .get("/users")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::ACCEPT_ENCODING, "gzip")
        .send()
        .await;
    compressed.assert_status_is_ok();
    compressed.assert_header(header::CONTENT_ENCODING, "gzip");

    let plain = app
        .http()
        .get("/users")
        .header(header::AUTHORIZATION, &bearer)
        .send()
        .await;
    plain.assert_status_is_ok();
    plain.assert_header_is_not_exist(header::CONTENT_ENCODING);
}

#[tokio::test]
async fn protected_route_rejects_a_missing_or_bogus_bearer_token() {
    let (_db, app) = boot().await;

    app.http()
        .get("/orgs")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    app.http()
        .get("/orgs")
        .header(header::AUTHORIZATION, "Bearer not-a-real-jwt")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn every_modelled_failure_returns_rfc9457_problem_json() {
    let (_db, app) = boot().await;
    let bearer = format!("Bearer {}", login().await);

    let unauthorized = app.http().get("/orgs").send().await;
    unauthorized.assert_status(StatusCode::UNAUTHORIZED);
    unauthorized.assert_header(header::CONTENT_TYPE, "application/problem+json");
    let body = unauthorized.json().await;
    let problem = body.value().object();
    assert_eq!(problem.get("status").i64(), 401);
    assert_eq!(problem.get("title").string(), "Unauthorized");
    assert!(
        problem.get_opt("type").is_some(),
        "problem carries a type URI"
    );

    let bad_request = app
        .http()
        .get("/users/not-a-uuid")
        .header(header::AUTHORIZATION, &bearer)
        .send()
        .await;
    bad_request.assert_status(StatusCode::BAD_REQUEST);
    bad_request.assert_header(header::CONTENT_TYPE, "application/problem+json");
    assert_eq!(
        bad_request
            .json()
            .await
            .value()
            .object()
            .get("status")
            .i64(),
        400,
    );

    let not_found = app
        .http()
        .get("/users/018f0000-0000-7000-8000-0000000000ff")
        .header(header::AUTHORIZATION, &bearer)
        .send()
        .await;
    not_found.assert_status(StatusCode::NOT_FOUND);
    not_found.assert_header(header::CONTENT_TYPE, "application/problem+json");
    assert_eq!(
        not_found.json().await.value().object().get("status").i64(),
        404,
    );

    create_org(&app, &bearer, "Conflict Co").await;
    let conflict = app
        .http()
        .post("/orgs")
        .header(header::AUTHORIZATION, &bearer)
        .body_json(&json!({ "name": "Conflict Co" }))
        .send()
        .await;
    conflict.assert_status(StatusCode::CONFLICT);
    conflict.assert_header(header::CONTENT_TYPE, "application/problem+json");
    let conflict_body = conflict.json().await;
    let conflict_problem = conflict_body.value().object();
    assert_eq!(conflict_problem.get("status").i64(), 409);
    assert_eq!(conflict_problem.get("title").string(), "Conflict");
}
