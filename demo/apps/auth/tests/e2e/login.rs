use features::{Claims, Role};
use nest_rs::http::StatusCode;
use serde_json::json;

use super::harness::*;

#[tokio::test]
async fn login_issues_a_token_the_public_key_verifies() {
    let (db, app) = boot().await;
    seed_org_and_user(db.connection().as_ref()).await;

    let resp = app
        .http()
        .post("/login")
        .body_json(&json!({
            "email": LOGIN_EMAIL,
            "password": LOGIN_PASSWORD,
        }))
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_header("cache-control", "no-store");
    resp.assert_header("pragma", "no-cache");

    let json = resp.json().await;
    let token = json
        .value()
        .object()
        .get("access_token")
        .string()
        .to_owned();
    let claims: Claims = resource_server_verifier()
        .verify(&token)
        .await
        .expect("the public key verifies the privately-signed token");
    assert_eq!(claims.org_id.to_string(), ORG_ID);
    assert!(claims.sub.is_some());
    assert!(claims.roles.contains(&Role::Admin));
}

#[tokio::test]
async fn login_rejects_bad_credentials() {
    let (db, app) = boot().await;
    seed_org_and_user(db.connection().as_ref()).await;

    let resp = app
        .http()
        .post("/login")
        .body_json(&json!({
            "email": LOGIN_EMAIL,
            "password": "wrong-password",
        }))
        .send()
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    resp.assert_json(json!({ "error": "invalid_grant" })).await;
}

#[tokio::test]
async fn login_rejects_unknown_email() {
    let (_db, app) = boot().await;

    let resp = app
        .http()
        .post("/login")
        .body_json(&json!({
            "email": "nobody@example.com",
            "password": LOGIN_PASSWORD,
        }))
        .send()
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    resp.assert_json(json!({ "error": "invalid_grant" })).await;
}

#[tokio::test]
async fn login_refuses_another_media_type_naming_only_json() {
    let (_db, app) = boot().await;

    let resp = app
        .http()
        .post("/login")
        .content_type(FOREIGN_MEDIA_TYPE)
        .body(
            json!({
                "email": LOGIN_EMAIL,
                "password": LOGIN_PASSWORD,
            })
            .to_string(),
        )
        .send()
        .await;
    resp.assert_status(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let body = resp.0.into_body().into_string().await.expect("a body");
    assert!(!body.contains("x-quoted-marker"), "{body}");
    assert!(body.contains("application/json"), "{body}");
}
