use features::authz::constants::{POSTS_READ, POSTS_WRITE};
use features::oauth::TARGET;
use features::{Claims, Role, Subject};
use nest_rs::http::StatusCode;
use nest_rs::testing::{LogCapture, TestApp, TestResponse};

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
        &format!(
            "grant_type=client_credentials&scope={POSTS_READ}+{POSTS_WRITE}&resource={}",
            encoded(API)
        ),
    )
    .await;
    resp.assert_status_is_ok();
    resp.assert_header("cache-control", "no-store");
    resp.assert_header("pragma", "no-cache");

    let json = resp.json().await;
    let obj = json.value().object();
    assert_eq!(obj.get("token_type").string(), "Bearer");
    assert!(obj.get("expires_in").i64() > 0);

    let claims: Claims = resource_server_verifier(API)
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

    let claims: Claims = resource_server_verifier(API)
        .verify(obj.get("access_token").string())
        .await
        .expect("the client's one resource is its tokens' audience");
    assert_eq!(claims.scopes, [POSTS_READ]);
    assert_eq!(claims.roles, [Role::User]);
    assert_eq!(claims.org_id.to_string(), ORG_ID);
    assert_eq!(claims.sub, Subject::Client(READER_ID.into()));
    assert_eq!(claims.client_id.as_deref(), Some(READER_ID));
}

#[tokio::test]
async fn token_endpoint_rejects_an_unsupported_grant() {
    let (_db, app) = boot().await;
    request_token(&app, (CLIENT_ID, CLIENT_SECRET), "grant_type=password")
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

fn encoded(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

#[tokio::test]
async fn a_token_names_the_resource_requested_and_no_other() {
    let (_db, app) = boot().await;

    let resp = request_token(
        &app,
        (CLIENT_ID, CLIENT_SECRET),
        &format!(
            "grant_type=client_credentials&resource={}",
            encoded(ASSISTANT)
        ),
    )
    .await;
    resp.assert_status_is_ok();
    let json = resp.json().await;
    let token = json
        .value()
        .object()
        .get("access_token")
        .string()
        .to_owned();

    resource_server_verifier(ASSISTANT)
        .verify::<Claims>(&token)
        .await
        .expect("the resource requested accepts it");
    assert!(
        resource_server_verifier(API)
            .verify::<Claims>(&token)
            .await
            .is_err(),
        "another resource the client is registered for refuses it"
    );
}

#[tokio::test]
async fn a_resource_outside_the_registration_or_none_among_several_is_an_invalid_target() {
    let logs = LogCapture::install();
    let (_db, app) = boot().await;

    for (client, body) in [
        (
            (READER_ID, READER_SECRET),
            format!(
                "grant_type=client_credentials&resource={}",
                encoded(ASSISTANT)
            ),
        ),
        (
            (CLIENT_ID, CLIENT_SECRET),
            "grant_type=client_credentials".to_owned(),
        ),
        (
            (CLIENT_ID, CLIENT_SECRET),
            format!(
                "grant_type=client_credentials&resource={}&resource={}",
                encoded(API),
                encoded(ASSISTANT)
            ),
        ),
    ] {
        assert_token_error(
            request_token(&app, client, &body).await,
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": "invalid_target" }),
        )
        .await;
    }
    let refusals: Vec<_> = logs
        .events()
        .into_iter()
        .filter(|event| event.target == TARGET && event.level == "warn")
        .collect();
    assert_eq!(refusals.len(), 3, "{refusals:?}");
    assert!(
        refusals
            .iter()
            .all(|event| event.field("reason").as_deref() == Some("invalid_target")),
        "{refusals:?}"
    );
}

async fn assert_token_error(resp: TestResponse, status: StatusCode, expected: serde_json::Value) {
    resp.assert_status(status);
    resp.assert_header("content-type", "application/json");
    resp.assert_header("cache-control", "no-store");
    resp.assert_header("pragma", "no-cache");
    resp.assert_json(expected).await;
}

#[tokio::test]
async fn token_endpoint_rejects_an_unauthenticated_client() {
    let (_db, app) = boot().await;

    for body in [
        "grant_type=client_credentials".to_owned(),
        format!(
            "grant_type=client_credentials&client_id={CLIENT_ID}&client_secret={CLIENT_SECRET}"
        ),
    ] {
        let resp = app
            .http()
            .post("/token")
            .content_type("application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await;
        resp.assert_header_is_not_exist("www-authenticate");
        assert_token_error(
            resp,
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": "invalid_client" }),
        )
        .await;
    }
}

#[tokio::test]
async fn token_endpoint_rejects_a_bad_client_secret() {
    let (_db, app) = boot().await;

    let resp = request_token(
        &app,
        (CLIENT_ID, "wrong-secret"),
        "grant_type=client_credentials",
    )
    .await;

    resp.assert_header("www-authenticate", r#"Basic realm="authorization server""#);
    assert_token_error(
        resp,
        StatusCode::UNAUTHORIZED,
        serde_json::json!({ "error": "invalid_client" }),
    )
    .await;
}

#[tokio::test]
async fn token_endpoint_refuses_a_request_without_a_grant_type() {
    let (_db, app) = boot().await;

    for body in ["scope=posts%3Aread", "grant_type=&scope=posts%3Aread"] {
        let resp = request_token(&app, (CLIENT_ID, CLIENT_SECRET), body).await;
        assert_token_error(
            resp,
            StatusCode::BAD_REQUEST,
            serde_json::json!({
                "error": "invalid_request",
                "error_description": "form field `grant_type`: missing",
            }),
        )
        .await;
    }
}

#[tokio::test]
async fn token_endpoint_refuses_a_scope_beyond_the_registration_naming_the_client_as_actor() {
    let logs = LogCapture::install();
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

    let refusal = logs.expect_one(TARGET, "token request refused");
    assert_eq!(refusal.field("reason").as_deref(), Some("invalid_scope"));
    assert_eq!(refusal.actor_id.as_deref(), Some(READER_ID));
    assert!(refusal.fields.keys().eq(["reason"]), "{refusal:?}");
}

#[tokio::test]
async fn token_endpoint_refuses_another_media_type_naming_only_the_form_type() {
    let (_db, app) = boot().await;

    let resp = app
        .http()
        .post("/token")
        .header("authorization", basic_auth(CLIENT_ID, CLIENT_SECRET))
        .content_type(FOREIGN_MEDIA_TYPE)
        .body("grant_type=client_credentials")
        .send()
        .await;
    assert_token_error(
        resp,
        StatusCode::BAD_REQUEST,
        serde_json::json!({
            "error": "invalid_request",
            "error_description":
                "the request's content type is not `application/x-www-form-urlencoded`",
        }),
    )
    .await;
}
