//! The transport edge (`src/edge.rs`) through the whole composed stack rather
//! than the layer alone: path normalization, and a handler that unwinds — in
//! process, and over real sockets for the connection it leaves serving.

use nest_rs_core::module;
use nest_rs_core::panic::{FIELD, LOCATION_FIELD};
use nest_rs_http::{HttpConfig, controller, routes};
use nest_rs_testing::LogCapture;
use poem::http::StatusCode;

use crate::transport::{PATIENCE, Serving, ask, connect, read_answer, request, serve_module};

#[controller(path = "/kitchen")]
struct KitchenController;

#[routes]
impl KitchenController {
    #[get("/")]
    async fn index(&self) -> &'static str {
        "kitchen"
    }

    #[get("/items/{id}")]
    async fn item(&self, id: nest_rs_http::Path<String>) -> String {
        format!("item {}", id.0)
    }

    #[get("/search")]
    async fn search(&self, req: &nest_rs_http::Request) -> String {
        req.uri().query().unwrap_or("none").to_owned()
    }

    #[get("/shelves/{n}")]
    async fn shelf(&self, n: nest_rs_http::Path<u32>) -> String {
        format!("shelf {}", n.0)
    }
}

#[module(providers = [KitchenController])]
struct KitchenModule;

#[tokio::test]
async fn a_collection_answers_with_and_without_the_trailing_slash() {
    let client = crate::boot::<KitchenModule>().await;

    let bare = client.get("/kitchen").send().await;
    bare.assert_status_is_ok();
    bare.assert_text("kitchen").await;

    let slashed = client.get("/kitchen/").send().await;
    slashed.assert_status_is_ok();
    slashed.assert_text("kitchen").await;
}

#[tokio::test]
async fn a_captured_segment_is_the_same_with_the_slash() {
    let client = crate::boot::<KitchenModule>().await;

    let slashed = client.get("/kitchen/items/42/").send().await;
    slashed.assert_status_is_ok();
    slashed.assert_text("item 42").await;
}

#[tokio::test]
async fn an_unmounted_path_still_answers_404() {
    let client = crate::boot::<KitchenModule>().await;

    let missing = client.get("/pantry/").send().await;
    missing.assert_status(StatusCode::NOT_FOUND);
    missing.assert_content_type("application/problem+json");
}

#[tokio::test]
async fn the_query_string_survives_normalization() {
    let client = crate::boot::<KitchenModule>().await;

    let resp = client.get("/kitchen/search/?q=1&sort=asc").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("q=1&sort=asc").await;
}

/// An `Err` escaping the route tree is rendered inside the header stamp, so the
/// router's own `404` and an extractor's `400` carry the security headers too.
#[tokio::test]
async fn an_error_the_route_tree_answers_carries_the_edge_s_headers() {
    let client = crate::boot::<KitchenModule>().await;

    let missing = client.get("/pantry").send().await;
    missing.assert_status(StatusCode::NOT_FOUND);
    missing.assert_content_type("application/problem+json");
    missing.assert_header("x-content-type-options", "nosniff");

    let unparsed = client.get("/kitchen/shelves/top").send().await;
    unparsed.assert_status(StatusCode::BAD_REQUEST);
    unparsed.assert_content_type("application/problem+json");
    unparsed.assert_header("x-content-type-options", "nosniff");
}

#[controller(path = "/oven")]
struct OvenController;

#[routes]
impl OvenController {
    #[get("/")]
    async fn bake(&self) -> &'static str {
        panic!("the oven caught fire")
    }

    #[get("/timer")]
    async fn timer(&self) -> &'static str {
        "ding"
    }
}

#[module(providers = [OvenController])]
struct OvenModule;

/// What the panic message says, which no answer may carry.
const FIRE: &str = "the oven caught fire";

/// The edge's one sentence for a handler it contained.
const ANSWERED: &str = "a handler panicked; answered 500";

#[nest_rs_core::main]
async fn ask_the_oven() -> (StatusCode, String) {
    let client = crate::boot::<OvenModule>().await;
    let resp = client.get("/oven").send().await;
    let status = resp.0.status();
    let body = resp.0.into_body().into_string().await.unwrap_or_default();
    (status, body)
}

/// The edge contains a handler's unwind under the process hook
/// `#[nest_rs::main]` installs: one event says it, the request's line files
/// `panic` with the `500` it answered, and the hook stays quiet.
#[test]
fn a_handler_that_panics_is_answered_500_and_said_once_beside_its_line() {
    let logs = LogCapture::install();

    let (status, body) = ask_the_oven();
    // Rust's own hook back, so an assertion failing below prints.
    drop(std::panic::take_hook());

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!body.contains(FIRE), "{body}");
    logs.expect_none(
        nest_rs_core::target::APP,
        "panicked where no unit of work contains it",
    );
    let said = logs.expect_one(nest_rs_http::target::HTTP, ANSWERED);
    assert_eq!(said.level, "error");
    assert_eq!(said.field(FIELD).as_deref(), Some(FIRE));
    let location = said.field(LOCATION_FIELD).unwrap_or_default();
    assert!(
        location.contains("tests/integration/edge.rs:"),
        "where it panicked: {said:#?}"
    );
    let line = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_http::unit::REQUEST.name(),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
        "{line:#?}"
    );
    assert_eq!(line.field("status").as_deref(), Some("500"), "{line:#?}");
    let span = logs.expect_span(nest_rs_http::target::HTTP, "http.request");
    assert_eq!(
        span.field("http.route").as_deref(),
        Some("/oven"),
        "{span:#?}"
    );
    assert_eq!(
        span.field("http.response.status_code").as_deref(),
        Some("500")
    );
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
        "the span says what the line says: {span:#?}"
    );
}

/// A panic answers its own request and leaves the connection it came on
/// serving the next one: keep-alive is the client's, not the handler's.
#[tokio::test]
async fn a_panic_leaves_the_keep_alive_connection_serving() {
    let logs = LogCapture::install();
    let Serving { port, cancel, .. } = serve_module::<OvenModule>(HttpConfig::default()).await;

    let mut stream = request(port, "/oven").await;
    let (head, body) = read_answer(&mut stream).await;
    assert!(head.starts_with("HTTP/1.1 500"), "{head}");
    assert!(
        head.to_ascii_lowercase()
            .contains("content-type: application/problem+json"),
        "{head}"
    );
    assert!(!body.contains(FIRE), "{body}");

    ask(&mut stream, "/oven/timer").await;
    let (head, body) = read_answer(&mut stream).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, "ding");
    logs.expect_none(nest_rs_http::target::HTTP, "a connection task panicked");
    cancel.cancel();
}

/// Over HTTP/2 the panicking stream is answered rather than reset, and the
/// connection opens the next one.
#[tokio::test]
async fn a_panic_answers_its_http2_stream_500_and_the_connection_keeps_serving() {
    let logs = LogCapture::install();
    let Serving { port, cancel, .. } = serve_module::<OvenModule>(HttpConfig::default()).await;
    let (client, connection) = h2::client::handshake(connect(port).await)
        .await
        .expect("the HTTP/2 preface is accepted");
    let connection = tokio::spawn(connection);
    let get = |path: &str| {
        poem::http::Request::get(format!("http://127.0.0.1:{port}{path}"))
            .body(())
            .expect("a request")
    };

    for (path, status) in [("/oven", 500), ("/oven/timer", 200)] {
        let mut ready = client.clone().ready().await.expect("a stream can open");
        let (answer, _) = ready.send_request(get(path), true).expect("sent");
        let response = tokio::time::timeout(PATIENCE, answer)
            .await
            .expect("the stream ends")
            .expect("answered, never reset");
        assert_eq!(response.status(), status, "{path}");
    }
    assert!(!connection.is_finished(), "the connection keeps serving");
    logs.expect_none(nest_rs_http::target::HTTP, "an HTTP/2 stream task panicked");
    cancel.cancel();
}
