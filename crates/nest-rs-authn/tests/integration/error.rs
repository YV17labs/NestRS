//! Covers `src/error.rs` — HTTP mapping for authentication failures, and
//! where a route's failure is filed.

use std::time::Duration;

use nest_rs_authn::AuthError;
use nest_rs_core::module;
use nest_rs_http::IntoResponse;
use nest_rs_http::{HttpModule, StatusCode, controller, header, routes};
use nest_rs_testing::{LogCapture, TestApp};

#[test]
fn maps_to_unauthorized_with_bearer_challenge() {
    // RFC 6750 §3.1: a rejected credential names its code, so a client can tell
    // "refresh and retry" from "start discovery".
    let response = AuthError::Expired.into_response();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some(r#"Bearer error="invalid_token", error_description="invalid token""#),
    );
}

/// §3, the other half: "if the request lacks any authentication information …
/// the resource server SHOULD NOT include an error code or other error
/// information". An unauthenticated probe has learned nothing yet.
#[test]
fn a_request_carrying_no_credentials_is_told_no_reason() {
    let response = AuthError::MissingCredentials.into_response();
    assert_eq!(
        response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Bearer"),
    );
}

#[tokio::test]
async fn failed_variant_body_hides_internal_detail() {
    let response = AuthError::failed("internal detail").into_response();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = response.into_body().into_string().await.unwrap();
    assert_eq!(body, "authentication failed");
}

#[controller(path = "/sign-in")]
struct SignInController;

#[routes]
impl SignInController {
    #[post("/outage")]
    #[public]
    async fn outage(&self) -> Result<String, AuthError> {
        Err(AuthError::unavailable_because(
            "the credential store is unreachable",
            Some(Duration::from_secs(2)),
            std::io::Error::other("connection refused by 10.0.0.1"),
        ))
    }

    #[post("/refused")]
    #[public]
    async fn refused(&self) -> Result<String, AuthError> {
        Err(AuthError::failed("the password does not match"))
    }
}

#[module(imports = [HttpModule::for_root(None)], providers = [SignInController])]
struct SignInModule;

/// Every `warn` and `error` a request filed: its level, its target, its
/// message and the chain it said.
fn said(logs: &LogCapture) -> Vec<(String, String, String, Option<String>)> {
    logs.events()
        .into_iter()
        .filter(|event| event.level == "warn" || event.level == "error")
        .map(|event| {
            let error = event.field("error");
            (event.level, event.target, event.message, error)
        })
        .collect()
}

/// An outage a handler answers with `?` is the `503` every edge files at
/// `warn`: once, by the HTTP edge, with its whole chain; the refused
/// credential beside it stays its own security line, the `401` the edge does
/// not file.
#[tokio::test]
async fn an_outage_a_handler_answers_is_filed_once_by_the_edge_at_warn() {
    let app = TestApp::for_module::<SignInModule>()
        .await
        .expect("the sign-in app boots");

    let logs = LogCapture::install();
    let response = app.http().post("/sign-in/outage").send().await;
    response.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    response.assert_header(header::RETRY_AFTER, "2");
    assert_eq!(
        said(&logs),
        [(
            "warn".to_owned(),
            nest_rs_http::target::HTTP.to_owned(),
            "request unavailable".to_owned(),
            Some(
                "authentication unavailable: the credential store is unreachable: connection refused by 10.0.0.1"
                    .to_owned()
            ),
        )],
    );

    let logs = LogCapture::install();
    app.http()
        .post("/sign-in/refused")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(
        said(&logs),
        [(
            "warn".to_owned(),
            nest_rs_authn::TARGET.to_owned(),
            "authentication failed".to_owned(),
            Some("authentication failed: the password does not match".to_owned()),
        )],
    );
}
