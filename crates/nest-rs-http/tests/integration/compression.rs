//! `HttpConfig.compression`: the transport negotiates response compression
//! from `Accept-Encoding`, and never encodes when the knob is off.

use std::time::Duration;

use nest_rs_core::module;
use nest_rs_http::{HttpConfig, controller, routes};
use poem::http::{StatusCode, header};
use poem::test::TestClient;

#[controller(path = "/echo")]
struct EchoController;

#[routes]
impl EchoController {
    #[get("/")]
    async fn echo(&self) -> String {
        "nestrs compression payload ".repeat(64)
    }

    #[get("/slow")]
    async fn slow(&self) -> String {
        tokio::time::sleep(Duration::from_secs(30)).await;
        "unreachable".into()
    }
}

#[module(providers = [EchoController])]
struct EchoModule;

async fn boot(
    compression: bool,
) -> TestClient<poem::endpoint::BoxEndpoint<'static, poem::Response>> {
    crate::boot_on::<EchoModule>(HttpConfig {
        compression,
        ..HttpConfig::default()
    })
    .await
}

#[tokio::test]
async fn compresses_when_the_client_accepts_gzip() {
    let client = boot(true).await;
    let resp = client
        .get("/echo")
        .header(header::ACCEPT_ENCODING, "gzip")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_header(header::CONTENT_ENCODING, "gzip");
}

#[tokio::test]
async fn leaves_a_non_accepting_client_uncompressed() {
    let client = boot(true).await;
    let resp = client.get("/echo").send().await;
    resp.assert_status_is_ok();
    resp.assert_header_is_not_exist(header::CONTENT_ENCODING);
}

#[tokio::test]
async fn compression_off_never_encodes_even_when_accepted() {
    let client = boot(false).await;
    let resp = client
        .get("/echo")
        .header(header::ACCEPT_ENCODING, "gzip")
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_header_is_not_exist(header::CONTENT_ENCODING);
}

#[tokio::test(start_paused = true)]
async fn a_timeout_under_compression_ships_a_decodable_problem_body() {
    // Compression runs inside the error boundary: it stamps `gzip` on the
    // timeout's `503` before the boundary rewrites the body uncompressed.
    let logs = nest_rs_testing::LogCapture::install();
    let client = crate::boot_on::<EchoModule>(HttpConfig {
        compression: true,
        request_timeout: Some(Duration::from_secs(1)),
        ..HttpConfig::default()
    })
    .await;
    let resp = client
        .get("/echo/slow")
        .header(header::ACCEPT_ENCODING, "gzip")
        .send()
        .await;
    assert_eq!(resp.0.status(), StatusCode::SERVICE_UNAVAILABLE);
    resp.assert_header(header::RETRY_AFTER, "1");

    let event = logs.expect_one("nest_rs::http", "request timed out");
    assert_eq!(event.level, "warn");
    assert!(
        event.field("timeout").is_some_and(|t| t.contains("1s")),
        "the event names the ceiling that fired, got {:?}",
        event.fields,
    );
    resp.assert_header_is_not_exist(header::CONTENT_ENCODING);
    let bytes = resp.0.into_body().into_bytes().await.expect("body");
    let problem: serde_json::Value = serde_json::from_slice(&bytes).expect("problem+json body");
    assert_eq!(problem["status"], 503);
}
