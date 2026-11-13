//! A request does not end when its handler returns: a streamed body runs under
//! the request's span and ambient context, and keeps its framing.

use std::time::Duration;

use nest_rs_core::{App, Transport, module};
use nest_rs_http::futures_util::{StreamExt, stream};
use nest_rs_http::{Body, Response};
use nest_rs_http::{HttpConfig, HttpModule, SseEvent, SseStream, controller, routes};
use nest_rs_testing::LogCapture;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use crate::boot;

/// What a body reports about the request it is being written under, from
/// inside the stream.
fn report(marker: &'static str) {
    tracing::info!(
        target: "test::body",
        ambient = nest_rs_core::current_trace_id().map(|id| id.to_hex()),
        span = tracing::Span::current().metadata().map(|meta| meta.name()),
        marker,
        "streamed",
    );
}

#[controller(path = "/stream")]
struct StreamController;

#[routes]
impl StreamController {
    /// An SSE route whose events are produced lazily, after this `async fn` returned.
    #[sse("/events")]
    #[public]
    async fn events(&self) -> SseStream {
        SseStream::new(stream::iter(["one", "two"]).map(|payload| {
            report("sse");
            SseEvent::message(payload)
        }))
    }

    /// The same gap without SSE.
    #[get("/chunks")]
    #[public]
    async fn chunks(&self) -> Response {
        Response::builder().body(Body::from_stream(stream::iter(["a", "b"]).map(|chunk| {
            report("chunks");
            Ok::<_, std::io::Error>(bytes::Bytes::from_static(chunk.as_bytes()))
        })))
    }

    /// A body whose length is known before it is written.
    #[get("/fixed")]
    #[public]
    async fn fixed(&self) -> String {
        String::from("hello")
    }

    /// An answer with no body at all.
    #[get("/empty")]
    #[public]
    async fn empty(&self) -> Response {
        Response::builder()
            .status(nest_rs_http::StatusCode::NO_CONTENT)
            .body(Body::empty())
    }
}

#[module(imports = [HttpModule], providers = [StreamController])]
struct StreamModule;

/// The access log is off: correlation must not depend on it.
#[module(imports = [HttpModule::for_root(
    HttpConfig { access_log: false, ..HttpConfig::default() },
)], providers = [StreamController])]
struct SilentStreamModule;

/// The id the client was told this request was filed under.
fn echoed_id(resp: &nest_rs_http::Response) -> String {
    let reported = resp
        .headers()
        .get("traceresponse")
        .and_then(|value| value.to_str().ok())
        .expect("the edge reports the trace it served the request under");
    nest_rs_core::TraceParent::parse(reported)
        .expect("and it is a valid traceparent")
        .trace_id
        .to_hex()
}

#[tokio::test]
async fn an_sse_stream_emits_under_the_request_that_opened_it() {
    let logs = LogCapture::install();
    let client = boot::<StreamModule>().await;

    let resp = client.get("/stream/events").send().await.0;
    let trace_id = echoed_id(&resp);
    // Draining the body is what runs the stream.
    let _ = resp.into_body().into_string().await.expect("the stream");

    let events = logs.find("test::body", "streamed");
    assert_eq!(events.len(), 2, "both items ran: {events:?}");
    for event in &events {
        assert_eq!(
            event.field("ambient").as_deref(),
            Some(trace_id.as_str()),
            "`current_trace_id()` inside the stream is the request's own: {:?}",
            event.fields,
        );
        assert_eq!(
            event.field("span").as_deref(),
            Some("http.request"),
            "the stream's events are rooted at the request span: {:?}",
            event.fields,
        );
    }
}

#[tokio::test]
async fn a_hand_built_streaming_body_is_carried_the_same_way() {
    let logs = LogCapture::install();
    let client = boot::<StreamModule>().await;

    let resp = client.get("/stream/chunks").send().await.0;
    let trace_id = echoed_id(&resp);
    let body = resp.into_body().into_string().await.expect("the stream");

    assert_eq!(body, "ab", "the wrapper does not reshape what is written");
    let events = logs.find("test::body", "streamed");
    assert_eq!(events.len(), 2, "both chunks ran: {events:?}");
    assert!(
        events
            .iter()
            .all(|event| event.field("ambient").as_deref() == Some(trace_id.as_str())),
        "an ordinary streaming body is the same family as `#[sse]`: {events:?}",
    );
}

#[tokio::test]
async fn a_stream_is_carried_with_the_access_log_switched_off() {
    let logs = LogCapture::install();
    let client = boot::<SilentStreamModule>().await;

    let resp = client.get("/stream/events").send().await.0;
    let trace_id = echoed_id(&resp);
    let _ = resp.into_body().into_string().await.expect("the stream");

    logs.expect_none(
        nest_rs_core::operation_log::TARGET,
        nest_rs_http::unit::REQUEST.name(),
    );
    let events = logs.find("test::body", "streamed");
    assert!(!events.is_empty(), "the stream ran");
    assert!(
        events
            .iter()
            .all(|event| event.field("ambient").as_deref() == Some(trace_id.as_str())),
        "the context is the request's, not the access log's: {events:?}",
    );
}

/// Read off the wire: hyper picks `Content-Length` over chunked from the body's
/// `size_hint`, which the wrapper must forward.
#[tokio::test]
async fn a_fixed_length_response_still_declares_its_length() {
    let app = App::builder()
        .module::<StreamModule>()
        .build()
        .await
        .expect("module boots");
    let mut transport = crate::built(&HttpConfig {
        host: "127.0.0.1".into(),
        port: 0,
        ..HttpConfig::default()
    });
    transport
        .configure(app.container())
        .await
        .expect("transport configures");
    let port = crate::listen(&mut transport).await;
    let cancel = CancellationToken::new();
    let serving = tokio::spawn({
        let cancel = cancel.clone();
        async move { Box::new(transport).serve(cancel).await }
    });

    let mut socket = connect(port).await;
    socket
        .write_all(b"GET /stream/fixed HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("the request is sent");
    socket.flush().await.expect("the request is flushed");
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut raw))
        .await
        .expect("the response arrives")
        .expect("the response reads");
    cancel.cancel();
    let _ = serving.await;

    let response = String::from_utf8_lossy(&raw).to_lowercase();
    assert!(
        response.contains("content-length: 5"),
        "a five-byte body declares its length: {response:?}",
    );
    assert!(
        !response.contains("transfer-encoding: chunked"),
        "and is not framed as a chunked stream: {response:?}",
    );
}

/// Retried because the transport binds asynchronously after `serve` is spawned.
async fn connect(port: u16) -> TcpStream {
    for _ in 0..100 {
        if let Ok(socket) = TcpStream::connect(("127.0.0.1", port)).await {
            return socket;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the transport never came up on {port}");
}

/// How many units have ended, as `record_end` tells the observability stack.
static ENDED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn count_end(_: &tracing::Span) {
    ENDED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Request `path` from an app booted on `M`, read its whole answer, and say
/// how many units ended meanwhile.
async fn ended_answering<M: nest_rs_core::Module + 'static>(path: &str) -> usize {
    nest_rs_core::__private::set_unit_end(count_end);
    let client = boot::<M>().await;
    let before = ENDED.load(std::sync::atomic::Ordering::SeqCst);
    let resp = client.get(path).send().await.0;
    let _ = resp.into_body().into_string().await.expect("the answer");
    ENDED.load(std::sync::atomic::Ordering::SeqCst) - before
}

/// A request that answered ends its unit with its answer, its status saying
/// how — so a duration read off its span stops there, whatever holds the span
/// open after — the access log on or off, its body empty or written.
#[tokio::test]
async fn a_request_that_answered_ends_its_unit_with_its_body() {
    assert_eq!(ended_answering::<StreamModule>("/stream/fixed").await, 1);
}

#[tokio::test]
async fn a_request_that_answered_ends_its_unit_with_the_access_log_switched_off() {
    assert_eq!(
        ended_answering::<SilentStreamModule>("/stream/fixed").await,
        1
    );
}

#[tokio::test]
async fn a_request_answered_with_no_body_ends_its_unit() {
    assert_eq!(ended_answering::<StreamModule>("/stream/empty").await, 1);
}

#[tokio::test]
async fn a_request_answered_with_no_body_ends_its_unit_with_the_access_log_switched_off() {
    assert_eq!(
        ended_answering::<SilentStreamModule>("/stream/empty").await,
        1
    );
}
