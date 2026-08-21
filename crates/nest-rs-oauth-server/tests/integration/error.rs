//! Covers `src/error.rs` — what a conforming OAuth client actually reads back
//! from a mounted token endpoint.
//!
//! The unit tests beside the enum assert the rendering; this asserts it
//! **through a booted app**, which is the only thing that proves the response
//! survives the transport it travels on. That distinction is not academic here:
//! an earlier version returned the code as a plain-text body, and the
//! resource-server interceptor stamped a `Bearer` challenge onto the `401` — so
//! a client authenticating with `Basic` was handed a pointer to RFC 9728
//! discovery instead of the reason its credentials were refused. Neither fault
//! is visible to a unit test of the error type.

use nest_rs_core::module;
use nest_rs_guards::NoBearerChallenge;
use nest_rs_http::{HttpModule, controller, routes};
use nest_rs_oauth_server::TokenError;
use nest_rs_testing::TestApp;
use poem::http::StatusCode;

#[controller(path = "/")]
struct TokenController;

#[routes]
impl TokenController {
    #[post("/token")]
    #[public]
    async fn token(&self) -> poem::Result<String> {
        Err(TokenError::UnsupportedGrant.into())
    }

    #[post("/token/client")]
    #[public]
    async fn client(&self) -> poem::Result<String> {
        Err(TokenError::InvalidClient.into())
    }
}

#[module(imports = [HttpModule::for_root(None)], providers = [TokenController])]
struct TokenApp;

/// RFC 6749 §5.2: "The parameters are included in the entity-body of the HTTP
/// response using the `application/json` media type", and §5.1's `no-store` /
/// `no-cache` bind the whole token endpoint.
#[tokio::test]
async fn a_refused_grant_reaches_the_client_as_the_rfc6749_json_error() {
    let app = TestApp::for_module::<TokenApp>()
        .await
        .expect("the token app boots");

    let response = app.http().post("/token").send().await;
    response.assert_status(StatusCode::BAD_REQUEST);
    response.assert_header("content-type", "application/json");
    response.assert_header("cache-control", "no-store");
    response.assert_header("pragma", "no-cache");
    response
        .assert_json(serde_json::json!({ "error": "unsupported_grant_type" }))
        .await;
}

/// §5.2 again, for the one condition that is a `401`: the code is
/// `invalid_client`, and the refusal must not be dressed up as a
/// oauth-resource challenge — which is what the `NoBearerChallenge` marker
/// on the response is for.
#[tokio::test]
async fn a_refused_client_is_a_401_that_is_not_a_resource_challenge() {
    let app = TestApp::for_module::<TokenApp>()
        .await
        .expect("the token app boots");

    let response = app.http().post("/token/client").send().await;
    response.assert_status(StatusCode::UNAUTHORIZED);
    response
        .assert_json(serde_json::json!({ "error": "invalid_client" }))
        .await;

    // Through `poem::Error`, which is what `?` in the handler above builds —
    // and the only spelling that reaches a client.
    let rendered = poem::Error::from(TokenError::InvalidClient).into_response();
    assert!(
        rendered.extensions().get::<NoBearerChallenge>().is_some(),
        "the response opts out of the RFC 9728 pointer",
    );
}
