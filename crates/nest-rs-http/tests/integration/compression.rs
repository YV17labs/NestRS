//! `HttpConfig.compression` — boot the real `App`, mount a controller, and pin
//! that the transport negotiates response compression from `Accept-Encoding`:
//! a `gzip` body when the client accepts it, plain bytes otherwise, and nothing
//! at all when the knob is off.

use std::time::Duration;

use nest_rs_core::{App, Transport, module};
use nest_rs_http::{HttpTransport, controller, routes};
use poem::http::{StatusCode, header};
use poem::test::TestClient;

#[controller(path = "/echo")]
struct EchoController;

#[routes]
impl EchoController {
    #[get("/")]
    async fn echo(&self) -> String {
        // A body long enough to be worth compressing; the middleware has no size
        // floor, but a realistic payload keeps the test honest.
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
    boot_with(compression, None).await
}

async fn boot_with(
    compression: bool,
    request_timeout: Option<Duration>,
) -> TestClient<poem::endpoint::BoxEndpoint<'static, poem::Response>> {
    let app = App::builder()
        .module::<EchoModule>()
        .build()
        .await
        .expect("module boots");
    let mut transport = HttpTransport::new().compression(compression);
    if let Some(timeout) = request_timeout {
        transport = transport.request_timeout(timeout);
    }
    transport
        .configure(app.container())
        .await
        .expect("transport configures against the live container");
    let endpoint = transport
        .take_endpoint()
        .expect("configure populates the endpoint");
    TestClient::new(endpoint)
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

#[tokio::test]
async fn a_timeout_under_compression_ships_a_decodable_problem_body() {
    // The compression layer runs inside the error boundary, so it stamps
    // `Content-Encoding: gzip` on the timeout's `Ok(504)` before the boundary
    // rewrites the body into (uncompressed) problem+json. The rewrite must drop
    // the stale encoding, or a browser (which always sends Accept-Encoding)
    // hits ERR_CONTENT_DECODING_FAILED.
    let client = boot_with(true, Some(Duration::from_millis(50))).await;
    let resp = client
        .get("/echo/slow")
        .header(header::ACCEPT_ENCODING, "gzip")
        .send()
        .await;
    assert_eq!(resp.0.status(), StatusCode::GATEWAY_TIMEOUT);
    resp.assert_header_is_not_exist(header::CONTENT_ENCODING);
    // The body is the real, uncompressed problem+json — parseable as-is.
    let bytes = resp.0.into_body().into_bytes().await.expect("body");
    let problem: serde_json::Value = serde_json::from_slice(&bytes).expect("problem+json body");
    assert_eq!(problem["status"], 504);
}
