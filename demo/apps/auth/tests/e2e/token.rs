use features::authz::constants::{POSTS_READ, POSTS_WRITE};
use features::{Claims, Role};
use nest_rs::testing::{TestApp, TestResponse};
use poem::http::StatusCode;

use super::harness::*;

async fn request_token(
    app: &TestApp,
    (client_id, client_secret): (&str, &str),
    body: &str,
) -> TestResponse {
    app.http()
        .post("/token")
        .header("authorization", basic_auth(client_id, client_secret))
        .content_type("application/x-www-form-urlencoded")
        .body(body.to_owned())
        .send()
        .await
}

#[tokio::test]
async fn token_endpoint_issues_a_token_the_public_key_verifies() {
    let (_db, app) = boot().await;

    let resp = request_token(
        &app,
        (CLIENT_ID, CLIENT_SECRET),
        &format!("grant_type=client_credentials&scope={POSTS_READ}+{POSTS_WRITE}"),
    )
    .await;
    resp.assert_status_is_ok();
    resp.assert_header("cache-control", "no-store");
    resp.assert_header("pragma", "no-cache");

    let json = resp.json().await;
    let obj = json.value().object();
    assert_eq!(obj.get("token_type").string(), "Bearer");
    assert!(obj.get("expires_in").i64() > 0);

    let claims: Claims = resource_server_verifier()
        .verify(obj.get("access_token").string())
        .await
        .expect("the public key verifies the privately-signed token");
    assert_eq!(claims.org_id.to_string(), ORG_ID);
    assert_eq!(claims.roles, [Role::Admin]);
    assert_eq!(claims.scopes, [POSTS_READ, POSTS_WRITE]);
}

#[tokio::test]
async fn a_client_asking_for_no_scope_is_granted_exactly_its_registration() {
    let (_db, app) = boot().await;

    let resp = request_token(
        &app,
        (READER_ID, READER_SECRET),
        "grant_type=client_credentials",
    )
    .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let obj = json.value().object();
    assert_eq!(obj.get("scope").string(), POSTS_READ);

    let claims: Claims = resource_server_verifier()
        .verify(obj.get("access_token").string())
        .await
        .expect("the public key verifies the privately-signed token");
    assert_eq!(claims.scopes, [POSTS_READ]);
    assert_eq!(claims.roles, [Role::User]);
    assert_eq!(claims.org_id.to_string(), ORG_ID);
    assert!(claims.sub.is_none());
}

#[tokio::test]
async fn token_endpoint_rejects_an_unsupported_grant() {
    let (_db, app) = boot().await;
    request_token(&app, (CLIENT_ID, CLIENT_SECRET), "grant_type=password")
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn token_endpoint_rejects_an_unauthenticated_client() {
    let (_db, app) = boot().await;
    app.http()
        .post("/token")
        .content_type("application/x-www-form-urlencoded")
        .body("grant_type=client_credentials")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn token_endpoint_rejects_a_bad_client_secret() {
    let (_db, app) = boot().await;
    request_token(
        &app,
        (CLIENT_ID, "wrong-secret"),
        "grant_type=client_credentials",
    )
    .await
    .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn token_endpoint_rejects_a_scope_beyond_the_client_registration() {
    let (_db, app) = boot().await;

    let resp = request_token(
        &app,
        (READER_ID, READER_SECRET),
        &format!("grant_type=client_credentials&scope={POSTS_WRITE}"),
    )
    .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    let json = resp.json().await;
    assert_eq!(json.value().object().get("error").string(), "invalid_scope");
}
