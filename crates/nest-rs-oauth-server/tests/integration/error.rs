//! Covers `src/error.rs` — what a conforming OAuth client actually reads back
//! from a mounted token endpoint.
//!
//! Booted, because the response must survive the transport: a resource-server
//! interceptor must not stamp a `Bearer` challenge onto the `401`.

use nest_rs_core::module;
use nest_rs_http::challenge::NoBearerChallenge;
use nest_rs_http::{HttpModule, StatusCode, controller, routes};
use nest_rs_oauth_server::{TARGET, TokenEndpointAuthMethod, TokenError};
use nest_rs_testing::{LogCapture, TestApp};

#[controller(path = "/")]
struct TokenController;

#[routes]
impl TokenController {
    #[post("/token")]
    #[public]
    async fn token(&self) -> nest_rs_http::Result<String> {
        Err(TokenError::UnsupportedGrant.into())
    }

    #[post("/token/client")]
    #[public]
    async fn client(&self) -> nest_rs_http::Result<String> {
        Err(TokenError::InvalidClient(TokenEndpointAuthMethod::ClientSecretBasic).into())
    }

    #[post("/token/client-in-body")]
    #[public]
    async fn client_in_body(&self) -> nest_rs_http::Result<String> {
        Err(TokenError::InvalidClient(TokenEndpointAuthMethod::ClientSecretPost).into())
    }

    #[post("/token/no-client")]
    #[public]
    async fn no_client(&self) -> nest_rs_http::Result<String> {
        Err(TokenError::InvalidClient(TokenEndpointAuthMethod::None).into())
    }

    #[post("/token/outage")]
    #[public]
    async fn outage(&self) -> nest_rs_http::Result<String> {
        Err(TokenError::Server(anyhow::anyhow!("identity store unreachable")).into())
    }

    #[post("/token/unsigned")]
    #[public]
    async fn unsigned(&self) -> nest_rs_http::Result<String> {
        Err(TokenError::Sign(anyhow::anyhow!("signing key unusable")).into())
    }
}

#[module(imports = [HttpModule::for_root(None)], providers = [TokenController])]
struct TokenApp;

/// RFC 6749 §5.2's `application/json` body, with §5.1's `no-store` and
/// `no-cache`; a refusal withholds nothing, so it files no failure line.
#[tokio::test]
async fn a_refused_grant_reaches_the_client_as_the_rfc6749_json_error() {
    let app = TestApp::for_module::<TokenApp>()
        .await
        .expect("the token app boots");
    let logs = LogCapture::install();

    let response = app.http().post("/token").send().await;
    response.assert_status(StatusCode::BAD_REQUEST);
    response.assert_header("content-type", "application/json");
    response.assert_header("cache-control", "no-store");
    response.assert_header("pragma", "no-cache");
    response
        .assert_json(serde_json::json!({ "error": "unsupported_grant_type" }))
        .await;
    assert!(
        logs.events().iter().all(|event| event.target != TARGET),
        "a client's refusal files nothing: {:#?}",
        logs.events(),
    );
}

/// The one `401`: a client refused over HTTP Basic, challenged with the
/// scheme it used (RFC 6749 §5.2), never dressed up as a resource challenge.
#[tokio::test]
async fn a_client_refused_over_basic_is_a_401_challenged_with_basic() {
    let app = TestApp::for_module::<TokenApp>()
        .await
        .expect("the token app boots");

    let response = app
        .http()
        .post("/token/client")
        .header("authorization", "Basic Y2k6d3Jvbmc=")
        .send()
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response.assert_header("www-authenticate", r#"Basic realm="authorization server""#);
    response
        .assert_json(serde_json::json!({ "error": "invalid_client" }))
        .await;

    // Through the `HttpError` `?` in the handler builds.
    let rendered = nest_rs_http::HttpError::from(TokenError::InvalidClient(
        TokenEndpointAuthMethod::ClientSecretBasic,
    ))
    .into_response();
    assert!(
        rendered.extensions().get::<NoBearerChallenge>().is_some(),
        "the response opts out of the RFC 9728 pointer",
    );
}

/// A client that never used the `Authorization` header — its credentials in
/// the body, or none — is §5.2's `400 invalid_client`, and carries no
/// challenge a browser would answer with a credentials prompt.
#[tokio::test]
async fn a_client_refused_without_http_authentication_is_a_400_carrying_no_challenge() {
    let app = TestApp::for_module::<TokenApp>()
        .await
        .expect("the token app boots");

    for path in ["/token/client-in-body", "/token/no-client"] {
        let response = app.http().post(path).send().await;
        response.assert_status(StatusCode::BAD_REQUEST);
        assert!(
            response.0.headers().get("www-authenticate").is_none(),
            "{path}: {:?}",
            response.0.headers(),
        );
        response
            .assert_json(serde_json::json!({ "error": "invalid_client" }))
            .await;
    }
}

/// A dependency that did not answer while the grant was resolved: `503` (RFC 9110
/// §15.6.4), the opaque code in the body and nothing of the cause, which the
/// operator reads once, at `warn`.
#[tokio::test]
async fn a_dependency_outage_is_a_503_naming_nothing_of_its_cause() {
    let app = TestApp::for_module::<TokenApp>()
        .await
        .expect("the token app boots");
    let logs = LogCapture::install();

    let response = app.http().post("/token/outage").send().await;
    response.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    response
        .assert_json(serde_json::json!({ "error": "server_error" }))
        .await;

    let filed = logs.expect_one(TARGET, "token request unavailable");
    assert_eq!(filed.level, "warn");
    let said = filed.field("error").unwrap_or_default();
    assert!(said.contains("identity store unreachable"), "{said}");
    logs.expect_none(TARGET, "token request failed");
}

/// A failure of this server's own — a token it could not sign — stays a `500`
/// answering the opaque code, its cause filed once at `error`.
#[tokio::test]
async fn a_signing_failure_is_a_500_whose_cause_reaches_the_operator_alone() {
    let app = TestApp::for_module::<TokenApp>()
        .await
        .expect("the token app boots");
    let logs = LogCapture::install();

    let response = app.http().post("/token/unsigned").send().await;
    response.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    response
        .assert_json(serde_json::json!({ "error": "server_error" }))
        .await;

    let filed = logs.expect_one(TARGET, "token request failed");
    assert_eq!(filed.level, "error");
    let said = filed.field("error").unwrap_or_default();
    assert!(said.contains("signing key unusable"), "{said}");
}
