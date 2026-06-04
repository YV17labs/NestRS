//! Covers `src/error.rs` — HTTP mapping for authentication failures.

use nestrs_authn::AuthError;
use poem::IntoResponse;
use poem::http::{StatusCode, header};

#[test]
fn maps_to_unauthorized_with_bearer_challenge() {
    let response = AuthError::Expired.into_response();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Bearer")
    );
}

#[tokio::test]
async fn failed_variant_body_hides_internal_detail() {
    let response = AuthError::Failed("internal detail".into()).into_response();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = response.into_body().into_string().await.unwrap();
    assert_eq!(body, "authentication failed");
}
