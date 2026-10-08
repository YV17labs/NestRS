//! The transport-wide request body cap: `max_body_bytes` bounds every
//! extractor at the transport edge, not only `RawBody`.

use std::io::Write;

use futures_util::StreamExt;
use nest_rs_core::module;
use nest_rs_http::{HttpTransport, RawBody, controller, routes};
use poem::http::{StatusCode, header};
use poem::test::TestClient;
use poem::web::Json;
use serde::Deserialize;

const CAP: usize = 64;

#[derive(Deserialize, schemars::JsonSchema)]
struct Payload {
    value: String,
}

#[controller(path = "/body")]
struct BodyController;

#[routes]
impl BodyController {
    #[post("/json")]
    async fn take_json(&self, body: Json<Payload>) -> String {
        body.0.value
    }

    #[post("/string")]
    async fn take_string(&self, body: String) -> String {
        format!("{} bytes", body.len())
    }

    #[post("/raw")]
    async fn take_raw(&self, body: RawBody) -> String {
        format!("{} bytes", body.len())
    }

    // A streamed body has no extractor limit: its only bound is the edge's.
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
    crate::boot_on::<BodyModule>(
        HttpTransport::new()
            .max_body_bytes(CAP)
            .compression(compression),
    )
    .await
}

fn oversized_json() -> Vec<u8> {
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

// poem's `Compression` decompresses before the edge and leaves the compressed
// `Content-Length` in the headers; the gzip is real so that middleware runs.
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
        .header(header::CONTENT_LENGTH, compressed.len().to_string())
        .header(header::CONTENT_ENCODING, "gzip")
        .body(compressed)
        .send()
        .await;

    resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
}

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
