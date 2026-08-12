//! B-HTTP-2 / HTTP-T4 — the transport-wide request body cap. `max_body_bytes`
//! must bound EVERY extractor at the transport edge, not only `RawBody`: a bare
//! `Json`/`String`/`Vec<u8>` handler used to buffer an attacker's multi-GB body
//! into OOM because the cap was installed only as a data extension the bare
//! extractors never read. These boot the real transport and drive it through
//! poem's `TestClient`.

use std::io::Write;

use futures_util::StreamExt;
use nest_rs_core::{App, Transport, module};
use nest_rs_http::{HttpTransport, RawBody, controller, routes};
use poem::http::{StatusCode, header};
use poem::test::TestClient;
use poem::web::Json;
use serde::Deserialize;

const CAP: usize = 64;

#[derive(Deserialize, schemars::JsonSchema)]
struct Payload {
    #[allow(dead_code)]
    value: String,
}

#[controller(path = "/body")]
struct BodyController;

#[routes]
impl BodyController {
    // A bare `Json<T>` — no `Valid<>`, no `RawBody`: the extractor that used to
    // buffer unbounded.
    #[post("/json")]
    async fn take_json(&self, body: Json<Payload>) -> String {
        body.0.value
    }

    // A bare `String` body extractor.
    #[post("/string")]
    async fn take_string(&self, body: String) -> String {
        format!("{} bytes", body.len())
    }

    // `RawBody` already honoured the cap — pin that it still does under the
    // transport-edge enforcement.
    #[post("/raw")]
    async fn take_raw(&self, body: RawBody) -> String {
        format!("{} bytes", body.len())
    }

    // The shape the buffering extractors' own limit never covers: a handler that
    // *streams* the body, as an upload going to object storage does. Whatever
    // bound it gets, it gets from the edge.
    #[post("/stream")]
    async fn take_stream(&self, body: poem::Body) -> poem::Result<String> {
        let mut stream = body.into_bytes_stream();
        let mut seen = 0usize;
        while let Some(chunk) = stream.next().await {
            seen += chunk.map_err(poem::error::InternalServerError)?.len();
        }
        Ok(format!("{seen} bytes"))
    }
}

#[module(providers = [BodyController])]
struct BodyModule;

async fn boot() -> TestClient<poem::endpoint::BoxEndpoint<'static, poem::Response>> {
    boot_with(false).await
}

async fn boot_with(
    compression: bool,
) -> TestClient<poem::endpoint::BoxEndpoint<'static, poem::Response>> {
    let app = App::builder()
        .module::<BodyModule>()
        .build()
        .await
        .expect("module boots");
    let mut transport = HttpTransport::new()
        .max_body_bytes(CAP)
        .compression(compression);
    transport
        .configure(app.container())
        .await
        .expect("transport configures against the live container");
    let endpoint = transport
        .take_endpoint()
        .expect("configure populates the endpoint");
    TestClient::new(endpoint)
}

fn oversized_json() -> Vec<u8> {
    // Well past the CAP once wrapped as JSON.
    format!(r#"{{"value":"{}"}}"#, "x".repeat(CAP * 4)).into_bytes()
}

#[tokio::test]
async fn bare_json_handler_rejects_an_oversized_body_with_413() {
    let client = boot().await;
    let resp = client
        .post("/body/json")
        .content_type("application/json")
        .body(oversized_json())
        .send()
        .await;
    resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn bare_string_handler_rejects_an_oversized_body_with_413() {
    let client = boot().await;
    let resp = client
        .post("/body/string")
        .body(vec![b'x'; CAP + 1])
        .send()
        .await;
    resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn raw_body_handler_rejects_an_oversized_body_with_413() {
    let client = boot().await;
    let resp = client
        .post("/body/raw")
        .body(vec![b'x'; CAP + 1])
        .send()
        .await;
    resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
}

/// The composition that defeated the cap. poem's `Compression` middleware wraps
/// the edge, so it decompresses the request body and hands the edge a body that
/// is nothing like the `Content-Length` still sitting in the headers. The edge
/// trusted that number — "the framing already bounds it" — and a streaming
/// consumer then had no bound at all: measured at 1026:1, a 64 KiB request wrote
/// a 64 MiB object under a 2 MiB cap.
///
/// The gzip is real rather than a hand-set header, because the header is the
/// symptom and the middleware is the cause.
#[tokio::test]
async fn a_compressed_body_cannot_outrun_the_cap_it_declares_it_is_under() {
    let client = boot_with(true).await;

    let payload = vec![b'x'; CAP * 100];
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(&payload).expect("gzip the payload");
    let compressed = encoder.finish().expect("finish the gzip stream");
    assert!(
        compressed.len() <= CAP,
        "the compressed body must fit under the cap for this to be the real case: {} bytes",
        compressed.len(),
    );

    let resp = client
        .post("/body/stream")
        // What the wire would carry: the *compressed* length, within the cap.
        .header(header::CONTENT_LENGTH, compressed.len().to_string())
        .header(header::CONTENT_ENCODING, "gzip")
        .body(compressed)
        .send()
        .await;

    // `413`, not merely "not 200": the cap is one declaration, so it answers
    // with one status whichever framing the caller chose and whichever extractor
    // happened to be reading when the count ran out.
    resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
}

/// The same request without the lie: a body that genuinely fits is streamed
/// through untouched, so the count above is a cap and not a ban on streaming.
#[tokio::test]
async fn a_streamed_body_within_the_cap_is_passed_through() {
    let client = boot().await;
    let resp = client
        .post("/body/stream")
        .header(header::CONTENT_LENGTH, CAP.to_string())
        .body(vec![b'x'; CAP])
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text(format!("{CAP} bytes")).await;
}

#[tokio::test]
async fn a_body_within_the_cap_is_accepted() {
    let client = boot().await;
    let resp = client
        .post("/body/json")
        .content_type("application/json")
        .body(br#"{"value":"ok"}"#.to_vec())
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.assert_text("ok").await;
}
