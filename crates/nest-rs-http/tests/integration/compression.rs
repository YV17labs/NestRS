//! `HttpConfig.compression` — boot the real `App`, mount a controller, and pin
//! that the transport negotiates response compression from `Accept-Encoding`:
//! a `gzip` body when the client accepts it, plain bytes otherwise, and nothing
//! at all when the knob is off.

use nest_rs_core::{App, Transport, module};
use nest_rs_http::{HttpTransport, controller, routes};
use poem::http::header;
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
}

#[module(providers = [EchoController])]
struct EchoModule;

async fn boot(
    compression: bool,
) -> TestClient<poem::endpoint::BoxEndpoint<'static, poem::Response>> {
    let app = App::builder()
        .module::<EchoModule>()
        .build()
        .await
        .expect("module boots");
    let mut transport = HttpTransport::new();
    transport = transport.compression(compression);
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
