//! `src/form.rs`: a posted `Form<T>` through a mounted route.

use nest_rs_http::StatusCode;
use nest_rs_http::{Form, FromRequest, HttpConfig, IntoResponse, Method};
use serde::Deserialize;

#[derive(Deserialize)]
struct Login {
    email: String,
}

async fn login_on(config: HttpConfig) -> crate::Client {
    crate::vocabulary_route_on(config, Method::POST, "/login", |req, mut body| async move {
        let Form(login) = Form::<Login>::from_request(&req, &mut body).await?;
        Ok(login.email.into_response())
    })
    .await
}

#[tokio::test]
async fn a_posted_form_decodes_and_another_body_is_415_never_quoting_its_type() {
    let client = login_on(HttpConfig::default()).await;
    let resp = client
        .post("/login")
        .form(&[("email", "ada@example.com")])
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("ada@example.com").await;

    let resp = client
        .post("/login")
        .content_type("text/plain; sk_live_51")
        .body("email=ada")
        .send()
        .await;
    resp.assert_status(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    resp.assert_content_type("application/problem+json");
    let body = resp.0.into_body().into_string().await.expect("a body");
    assert!(!body.contains("sk_live"), "{body}");
}

#[tokio::test]
async fn a_form_past_the_transport_cap_is_413() {
    let client = login_on(HttpConfig {
        max_body_bytes: Some(16),
        ..HttpConfig::default()
    })
    .await;
    client
        .post("/login")
        .form(&[("email", "a-long-address@example.com")])
        .send()
        .await
        .assert_status(StatusCode::PAYLOAD_TOO_LARGE);
}
