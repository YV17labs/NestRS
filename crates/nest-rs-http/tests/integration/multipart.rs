//! `src/multipart.rs`: `Multipart` read through a mounted route, under the
//! transport's cap.

use nest_rs_http::StatusCode;
use nest_rs_http::{FromRequest, HttpConfig, IntoResponse, Method, Multipart};
use nest_rs_testing::TestForm;

async fn uploads_on(config: HttpConfig) -> crate::Client {
    crate::vocabulary_route_on(
        config,
        Method::POST,
        "/uploads",
        |req, mut body| async move {
            let mut form = Multipart::from_request(&req, &mut body).await?;
            let mut read = Vec::new();
            while let Some(field) = form.next_field().await? {
                let name = field.name().unwrap_or_default().to_owned();
                let bytes = field.bytes().await?;
                read.push(format!("{name}={}", bytes.len()));
            }
            Ok(read.join(" ").into_response())
        },
    )
    .await
}

#[tokio::test]
async fn each_part_arrives_with_its_name_and_bytes() {
    let client = uploads_on(HttpConfig::default()).await;
    let form = TestForm::new()
        .text("title", "Hello")
        .bytes("file", vec![7_u8; 1024]);
    let resp = client.post("/uploads").multipart(form).send().await;
    resp.assert_status_is_ok();
    resp.assert_text("title=5 file=1024").await;
}

#[tokio::test]
async fn a_multipart_field_past_the_cap_is_413() {
    let client = uploads_on(HttpConfig {
        max_body_bytes: Some(4096),
        ..HttpConfig::default()
    })
    .await;
    let form = TestForm::new().bytes("file", vec![7_u8; 64 * 1024]);
    client
        .post("/uploads")
        .multipart(form)
        .send()
        .await
        .assert_status(StatusCode::PAYLOAD_TOO_LARGE);
}
