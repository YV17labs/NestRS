//! Covers `src/error.rs` — HTTP mapping for authentication failures.

use nest_rs_authn::AuthError;
use poem::IntoResponse;
use poem::http::{StatusCode, header};

#[test]
fn maps_to_unauthorized_with_bearer_challenge() {
    // RFC 6750 §3.1: a rejected credential names which of the three codes
    // applies, and §3 requires the scheme carry at least one auth-param. An
    // expired token and an absent one used to produce byte-identical `401`s,
    // so a client could not tell "refresh and retry" from "start discovery".
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
    let response = AuthError::Failed("internal detail".into()).into_response();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = response.into_body().into_string().await.unwrap();
    assert_eq!(body, "authentication failed");
}
