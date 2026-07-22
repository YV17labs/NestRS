use features::{Claims, Role};
use poem::http::StatusCode;

use super::harness::*;

#[tokio::test]
async fn token_endpoint_issues_a_token_the_public_key_verifies() {
    let (_db, app) = boot().await;

    let resp = app
        .http()
        .post("/token")
        .header("authorization", basic_auth(CLIENT_ID, CLIENT_SECRET))
        .content_type("application/x-www-form-urlencoded")
        .body("grant_type=client_credentials&scope=admin+user")
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let obj = json.value().object();
    assert_eq!(obj.get("token_type").string(), "Bearer");
    assert!(obj.get("expires_in").i64() > 0);
    let token = obj.get("access_token").string().to_owned();

    let claims: Claims = resource_server_verifier()
        .verify(&token)
        .expect("the public key verifies the privately-signed token");
    assert_eq!(claims.org_id.to_string(), ORG_ID);
    assert!(claims.roles.contains(&Role::Admin));
}

#[tokio::test]
async fn token_endpoint_rejects_an_unsupported_grant() {
    let (_db, app) = boot().await;
    app.http()
        .post("/token")
        .header("authorization", basic_auth(CLIENT_ID, CLIENT_SECRET))
        .content_type("application/x-www-form-urlencoded")
        .body("grant_type=password")
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn token_endpoint_rejects_an_unauthenticated_client() {
    let (_db, app) = boot().await;
    app.http()
        .post("/token")
        .content_type("application/x-www-form-urlencoded")
        .body("grant_type=client_credentials&scope=user")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn token_endpoint_rejects_a_bad_client_secret() {
    let (_db, app) = boot().await;
    app.http()
        .post("/token")
        .header("authorization", basic_auth(CLIENT_ID, "wrong-secret"))
        .content_type("application/x-www-form-urlencoded")
        .body("grant_type=client_credentials&scope=user")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn token_endpoint_rejects_a_scope_beyond_the_client_grant() {
    let (_db, app) = boot().await;
    app.http()
        .post("/token")
        .header(
            "authorization",
            basic_auth("limited-service", "limited-secret"),
        )
        .content_type("application/x-www-form-urlencoded")
        .body("grant_type=client_credentials&scope=admin")
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn token_endpoint_derives_the_org_from_the_authenticated_client() {
    let (_db, app) = boot().await;
    let resp = app
        .http()
        .post("/token")
        .header(
            "authorization",
            basic_auth("limited-service", "limited-secret"),
        )
        .content_type("application/x-www-form-urlencoded")
        .body("grant_type=client_credentials")
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let token = json
        .value()
        .object()
        .get("access_token")
        .string()
        .to_owned();
    let claims: Claims = resource_server_verifier()
        .verify(&token)
        .expect("the public key verifies the privately-signed token");
    assert_eq!(claims.org_id.to_string(), ORG_ID);
    assert_eq!(claims.roles, vec![Role::User]);
}
