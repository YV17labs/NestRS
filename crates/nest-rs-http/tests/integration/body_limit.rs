//! B-HTTP-2 / HTTP-T4 — the transport-wide request body cap. `max_body_bytes`
//! must bound EVERY extractor at the transport edge, not only `RawBody`: a bare
//! `Json`/`String`/`Vec<u8>` handler used to buffer an attacker's multi-GB body
//! into OOM because the cap was installed only as a data extension the bare
//! extractors never read. These boot the real transport and drive it through
//! poem's `TestClient`.

use nest_rs_core::{App, Transport, module};
use nest_rs_http::{HttpTransport, RawBody, controller, routes};
use poem::http::StatusCode;
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
}

#[module(providers = [BodyController])]
struct BodyModule;

async fn boot() -> TestClient<poem::endpoint::BoxEndpoint<'static, poem::Response>> {
    let app = App::builder()
        .module::<BodyModule>()
        .build()
        .await
        .expect("module boots");
    let mut transport = HttpTransport::new().max_body_bytes(CAP);
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
