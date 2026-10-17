//! `HttpConfig.compression`: the transport negotiates response compression
//! from `Accept-Encoding` as RFC 9110 §12.5.3 reads it — weights, `identity`,
//! a coding refused with `q=0` — leaves an event stream as it is written, and
//! never encodes when the knob is off.

use std::time::Duration;

use nest_rs_core::module;
use nest_rs_http::{HttpConfig, SseEvent, SseStream, controller, routes};
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

    /// The first part of a 4000-byte representation, as a range request is
    /// answered.
    #[get("/part")]
    async fn part(&self) -> nest_rs_http::Response {
        nest_rs_http::Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(
                header::CONTENT_RANGE,
                nest_rs_http::HeaderValue::from_static("bytes 0-1727/4000"),
            )
            .body("x".repeat(1728))
    }

    #[sse("/events")]
    async fn events(&self) -> SseStream {
        SseStream::new(futures_util::stream::iter([SseEvent::message("one")]))
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

/// The `Content-Encoding` the transport chose for a client sending
/// `accept_encoding`, if any.
async fn chosen(path: &str, accept_encoding: &str) -> Option<String> {
    let client = boot(true).await;
    let resp = client
        .get(path)
        .header(header::ACCEPT_ENCODING, accept_encoding)
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.0
        .headers()
        .get(header::CONTENT_ENCODING)
        .map(|value| value.to_str().expect("visible ASCII").to_owned())
}

#[tokio::test]
async fn the_coding_with_the_highest_weight_is_chosen() {
    assert_eq!(
        chosen("/echo", "gzip;q=0.2, deflate;q=0.9")
            .await
            .as_deref(),
        Some("deflate")
    );
    assert_eq!(
        chosen("/echo", "gzip; q=0.2, deflate ; q=0.9")
            .await
            .as_deref(),
        Some("deflate"),
        "whitespace around the weight is the grammar's (OWS)",
    );
}

#[tokio::test]
async fn identity_weighted_above_every_coding_is_answered_unencoded() {
    assert_eq!(chosen("/echo", "gzip;q=0.5, identity;q=1").await, None);
}

#[tokio::test]
async fn a_coding_refused_with_a_zero_weight_is_never_used() {
    assert_eq!(chosen("/echo", "gzip;q=0").await, None);
    assert_eq!(chosen("/echo", "*;q=0, identity").await, None);
}

#[tokio::test]
async fn an_answer_compression_could_encode_varies_on_accept_encoding() {
    let client = boot(true).await;
    for accept_encoding in ["gzip", "identity"] {
        let resp = client
            .get("/echo")
            .header(header::ACCEPT_ENCODING, accept_encoding)
            .send()
            .await;
        let vary: Vec<String> = resp
            .0
            .headers()
            .get_all(header::VARY)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(',').map(|v| v.trim().to_ascii_lowercase()))
            .collect();
        assert!(
            vary.iter().any(|field| field == "accept-encoding"),
            "{accept_encoding}: {vary:?}"
        );
    }
}

/// `Content-Range` counts bytes of the representation as selected, before
/// any content coding (RFC 9110 §14.4): an encoded part would name offsets
/// its body does not have.
#[tokio::test]
async fn a_partial_answer_is_never_compressed() {
    let client = boot(true).await;
    let resp = client
        .get("/echo/part")
        .header(header::ACCEPT_ENCODING, "gzip")
        .send()
        .await;
    resp.assert_status(StatusCode::PARTIAL_CONTENT);
    resp.assert_header(header::CONTENT_RANGE, "bytes 0-1727/4000");
    resp.assert_header_is_not_exist(header::CONTENT_ENCODING);
}

#[tokio::test]
async fn an_event_stream_is_never_compressed() {
    assert_eq!(
        chosen("/echo/events", "gzip").await,
        None,
        "a compressor holds each event back until its buffer fills",
    );
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
