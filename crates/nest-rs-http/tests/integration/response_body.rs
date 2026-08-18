//! A request does not end when its handler returns.
//!
//! Every response here streams: the handler hands back a body and returns, and
//! the body's own code runs afterwards, on the connection task, with the
//! transport edge's task-locals already unwound and its span already exited.
//! What is asserted is that the framework carries the request into that gap —
//! the span, so the stream's events name the request, and the ambient context,
//! so the developer's own `current_trace_id()` answers there too.
//!
//! `#[sse]` is the loudest member of that family and the one a developer meets
//! first, so it is a fixture here; a hand-built streaming body is the other, and
//! both go through the same wrapper. The framing test at the bottom belongs to
//! the same wrapper for the opposite reason: it is what the wrapper must **not**
//! change.

use std::net::TcpListener as StdTcpListener;
use std::time::Duration;

use nest_rs_core::{App, Transport, module};
use nest_rs_http::futures_util::{StreamExt, stream};
use nest_rs_http::{
    HttpConfig, HttpModule, HttpTransport, SseEvent, SseStream, controller, routes,
};
use nest_rs_testing::LogCapture;
use poem::{Body, IntoResponse, Response};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use crate::boot;

/// What a body reports about the request it is being written under. Emitted
/// from *inside* the stream, which is the whole point: nothing else in this
/// suite runs there.
fn report(marker: &'static str) {
    tracing::info!(
        target: "test::body",
        // The developer's half — a task-local an `#[sse]` handler's stream, a
        // download or a `Repo` call inside one would read.
        ambient = nest_rs_core::current_trace_id().map(|id| id.to_hex()),
        // The framework's half — what puts `trace_id` on events the stream's
        // author never thought to add a field to.
        span = tracing::Span::current().metadata().map(|meta| meta.name()),
        marker,
        "streamed",
    );
}

#[controller(path = "/stream")]
struct StreamController;

#[routes]
impl StreamController {
    /// The reachable case: an SSE route whose events are produced lazily, one
    /// poll at a time, long after this `async fn` has returned.
    #[sse("/events")]
    #[public]
    async fn events(&self) -> SseStream {
        SseStream::new(stream::iter(["one", "two"]).map(|payload| {
            report("sse");
            SseEvent::message(payload)
        }))
    }

    /// The same gap without SSE: any handler may return a streaming body, and
    /// what closes it is the body wrapper rather than anything `#[sse]` owns.
    #[get("/chunks")]
    #[public]
    async fn chunks(&self) -> Response {
        Body::from_bytes_stream(stream::iter(["a", "b"]).map(|chunk| {
            report("chunks");
            Ok::<_, std::io::Error>(bytes::Bytes::from_static(chunk.as_bytes()))
        }))
        .into_response()
    }

    /// A body whose length is known before it is written — the case the framing
    /// test reads off the wire.
    #[get("/fixed")]
    #[public]
    async fn fixed(&self) -> String {
        String::from("hello")
    }
}

#[module(imports = [HttpModule::for_root(None)], providers = [StreamController])]
struct StreamModule;

/// The access log is deliberately **off**. Correlation is the framework's
/// primitive, so what a stream can answer about itself must not depend on
/// whether an operator wanted a log line — wiring the body only when the line is
/// on is exactly the shape that would make this true for some deployments and
/// false for others.
#[module(imports = [HttpModule::for_root(
    HttpConfig { access_log: false, ..HttpConfig::default() },
)], providers = [StreamController])]
struct SilentStreamModule;

/// The id the client was told this request was filed under. Every assertion
/// below compares against it rather than against a captured value, so a stream
/// running under *some* id it invented would not pass.
fn echoed_id(resp: &poem::Response) -> String {
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
    // Draining the body is what runs the stream — before this line the handler
    // has returned and not one event has been produced.
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

    logs.expect_none("nest_rs::access", "request served");
    let events = logs.find("test::body", "streamed");
    assert!(!events.is_empty(), "the stream ran");
    assert!(
        events
            .iter()
            .all(|event| event.field("ambient").as_deref() == Some(trace_id.as_str())),
        "the context is the request's, not the access log's: {events:?}",
    );
}

/// The half the wrapper must **not** change, and the one that has to be read off
/// the wire: hyper picks `Content-Length` over `Transfer-Encoding: chunked` from
/// the body's `size_hint`, so a wrapper that forwards no hint turns every
/// response in the framework chunked — silently, and invisibly to any assertion
/// made on a `Response` before it is written.
#[tokio::test]
async fn a_fixed_length_response_still_declares_its_length() {
    let app = App::builder()
        .module::<StreamModule>()
        .build()
        .await
        .expect("module boots");
    let listener = StdTcpListener::bind(("127.0.0.1", 0)).expect("bind an ephemeral port");
    let port = listener.local_addr().expect("the bound port").port();
    drop(listener);

    let mut transport = HttpTransport::new().bind(format!("127.0.0.1:{port}"));
    transport
        .configure(app.container())
        .await
        .expect("transport configures");
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
