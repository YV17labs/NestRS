//! `#[sse]` on a running route: the media type, the frames, the ceiling, a peer
//! that stops reading, and a stream that unwinds. Its compile-time refusals are
//! trybuild snapshots in `nest-rs-macro-hygiene`.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{AmbientValue, Layer, ambient, injectable, module, with_ambient};
use nest_rs_http::futures_util::StreamExt;
use nest_rs_http::{
    HttpConfig, HttpModule, Scoped, SseEvent, SseStream, async_trait, controller,
    futures_util::stream, routes,
};
use nest_rs_interceptors::{Interceptor, Next};
use nest_rs_testing::LogCapture;
use tokio::io::AsyncWriteExt;

use crate::boot;
use crate::transport::{
    PATIENCE, Serving, after, connect, read_head, read_to_end, request, serve_module, small_window,
    still_open,
};

#[controller(path = "/feed")]
struct FeedController;

#[routes]
impl FeedController {
    /// Three events, then the stream ends on its own.
    #[sse("/ticks")]
    #[public]
    async fn ticks(&self) -> SseStream {
        SseStream::new(stream::iter([
            SseEvent::message("one").event_type("tick").id("1"),
            SseEvent::message("two").event_type("tick").id("2"),
            SseEvent::message("three").event_type("tick").id("3"),
        ]))
    }

    /// A fallible open whose `Result` is spelled through an alias.
    #[sse("/closed")]
    #[public]
    async fn closed(&self) -> nest_rs_http::Result<SseStream> {
        Err(nest_rs_http::HttpError::from_status(
            nest_rs_http::StatusCode::SERVICE_UNAVAILABLE,
        ))
    }

    /// The same alias, opening.
    #[sse("/open")]
    #[public]
    async fn open(&self) -> Opened {
        Ok(SseStream::new(stream::iter([SseEvent::message("one")])))
    }

    /// A stream that never ends; only the ceiling closes it.
    #[sse("/forever")]
    #[public]
    async fn forever(&self) -> SseStream {
        SseStream::new(stream::pending())
    }
}

/// A fallible open under another name.
type Opened = Result<SseStream, nest_rs_http::HttpError>;

/// A one-second ceiling, pinned on the module as a deployment would.
#[module(imports = [HttpModule::for_root(
    HttpConfig { sse_max_connection: Some(Duration::from_secs(1)), ..HttpConfig::default() },
)], providers = [FeedController])]
struct FeedModule;

#[tokio::test]
async fn an_sse_route_answers_text_event_stream() {
    let client = boot::<FeedModule>().await;
    let resp = client.get("/feed/ticks").send().await;
    resp.assert_status_is_ok();
    // poem appends the charset, so only the media type's prefix is compared.
    let content_type = resp
        .0
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        content_type.starts_with("text/event-stream"),
        "an `#[sse]` route answers `text/event-stream`, got {content_type:?}",
    );
}

#[tokio::test]
async fn a_fallible_open_is_known_by_its_type_whatever_it_is_called() {
    let client = boot::<FeedModule>().await;
    client
        .get("/feed/closed")
        .send()
        .await
        .assert_status(nest_rs_http::StatusCode::SERVICE_UNAVAILABLE);
    let resp = client.get("/feed/open").send().await;
    resp.assert_status_is_ok();
    let body = resp.0.into_body().into_string().await.unwrap_or_default();
    assert!(body.contains("data: one"), "the stream opens: {body:?}");
}

#[tokio::test]
async fn the_events_reach_the_client_with_their_type_and_id() {
    let client = boot::<FeedModule>().await;
    let body = client.get("/feed/ticks").send().await.0.into_body();
    let text = body.into_string().await.expect("the stream is text");
    assert!(
        text.contains("event: tick"),
        "event type is framed: {text:?}"
    );
    assert!(text.contains("data: one"), "payloads are framed: {text:?}");
    assert!(
        text.contains("id: 3"),
        "an id a reconnecting client sends back as `Last-Event-ID` is framed: {text:?}",
    );
}

#[tokio::test(start_paused = true)]
async fn the_connection_ceiling_closes_a_stream_that_never_ends() {
    let client = boot::<FeedModule>().await;
    // The assertion is that this returns at all: only the ceiling ends the stream.
    let served = tokio::time::timeout(Duration::from_secs(20), async {
        client
            .get("/feed/forever")
            .send()
            .await
            .0
            .into_body()
            .into_string()
            .await
    })
    .await
    .expect("the ceiling ends the stream well inside the timeout")
    .expect("the stream is text");
    assert!(
        served.is_empty(),
        "a pending stream emits nothing before the ceiling closes it, got {served:?}",
    );
}

/// The ceiling the streaming route below is mounted with.
const CEILING: Duration = Duration::from_secs(1);

/// The send deadline the stream below is served under.
const SEND: Duration = Duration::from_secs(2);

#[controller(path = "/flood")]
struct FloodController;

#[routes]
impl FloodController {
    /// Emits as fast as it is polled, in chunks big enough to park the write.
    #[sse("/events")]
    #[public]
    async fn events(&self) -> SseStream {
        SseStream::new(stream::repeat_with(|| {
            SseEvent::message("x".repeat(16 * 1024))
        }))
    }
}

#[module(imports = [HttpModule::for_root(
    HttpConfig { sse_max_connection: Some(CEILING), ..HttpConfig::default() },
)], providers = [FloodController])]
struct FloodModule;

/// The ceiling bounds emission; a peer that stops reading parks the write,
/// and the send deadline drops the connection, freeing its place under the cap
/// unasked, with one `warn`.
#[tokio::test]
async fn a_peer_that_stops_reading_an_event_stream_is_cut_at_the_send_deadline() {
    let logs = LogCapture::install();
    let Serving { port, cancel, .. } = serve_module::<FloodModule>(HttpConfig {
        max_concurrent_connections: 1,
        send_timeout: SEND,
        ..HttpConfig::default()
    })
    .await;
    let mut socket = small_window(port).await;
    socket
        .write_all(
            b"GET /flood/events HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n",
        )
        .await
        .expect("the request is sent");
    let head = tokio::time::timeout(PATIENCE, read_head(&mut socket))
        .await
        .expect("the stream opens");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");

    after(3 * SEND).await;
    let mut next = request(port, "/flood/missing").await;
    let head = tokio::time::timeout(PATIENCE, read_head(&mut next))
        .await
        .expect("answered once the stalled stream's connection is gone");
    assert!(head.starts_with("HTTP/1.1 404"), "{head}");
    let cut = logs.expect_one(
        nest_rs_http::target::HTTP,
        "the peer took nothing within the send deadline; it is cut off",
    );
    assert_eq!(cut.level, "warn");
    cancel.cancel();
}

#[controller(path = "/flaky")]
struct FlakyFeedController;

#[routes]
impl FlakyFeedController {
    /// One event, then a panic as the stream is polled for the next — after
    /// two waits, so the event is on the wire first: on HTTP/2 the transport
    /// polls a body once past the frame it hands, to tell its end.
    #[sse("/events")]
    #[public]
    async fn events(&self) -> SseStream {
        let mut polls = 0;
        SseStream::new(stream::poll_fn(move |cx| {
            polls += 1;
            match polls {
                1 => std::task::Poll::Ready(Some(SseEvent::message("one"))),
                2 | 3 => {
                    cx.waker().wake_by_ref();
                    std::task::Poll::Pending
                }
                _ => panic!("the stream panicked"),
            }
        }))
    }

    /// A stream with nothing to say, open on another connection meanwhile.
    #[sse("/quiet")]
    #[public]
    async fn quiet(&self) -> SseStream {
        SseStream::new(stream::pending())
    }

    /// An ordinary answer, asked on the connection a stream unwound on.
    #[get("/ping")]
    #[public]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

#[module(providers = [FlakyFeedController])]
struct FlakyFeedModule;

/// The body's one sentence for a stream it contained.
const ENDED: &str = "a response body panicked; it ended unfinished";

/// A stream that unwinds ends its own body — over HTTP/1 the connection under
/// it, whose framing cannot be finished — files its line `panic`, and touches
/// no other connection.
#[tokio::test]
async fn a_stream_that_panics_ends_its_body_and_files_panic() {
    let logs = LogCapture::install();
    let Serving { port, cancel, .. } = serve_module::<FlakyFeedModule>(HttpConfig::default()).await;
    let mut quiet = request(port, "/flaky/quiet").await;
    let head = tokio::time::timeout(PATIENCE, read_head(&mut quiet))
        .await
        .expect("the quiet stream opens");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");

    let mut flaky = request(port, "/flaky/events").await;
    let head = tokio::time::timeout(PATIENCE, read_head(&mut flaky))
        .await
        .expect("the flaky stream opens");
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let rest = read_to_end(&mut flaky).await;
    assert!(rest.contains("data: one"), "the event before it: {rest:?}");
    assert!(
        !rest.ends_with("0\r\n\r\n"),
        "never framed as a stream that ended: {rest:?}"
    );

    assert!(
        still_open(&mut quiet).await,
        "the other connection serves on"
    );
    let said = logs.expect_one(nest_rs_http::target::HTTP, ENDED);
    assert_eq!(
        said.field(nest_rs_core::panic::FIELD).as_deref(),
        Some("the stream panicked")
    );
    let lines: Vec<_> = logs
        .find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_http::unit::REQUEST.name(),
        )
        .into_iter()
        .filter(|line| line.field("route").as_deref() == Some("/flaky/events"))
        .collect();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert_eq!(
        lines[0].field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
        "{:#?}",
        lines[0]
    );
    assert_eq!(lines[0].field("status").as_deref(), Some("200"));
    logs.expect_none(nest_rs_http::target::HTTP, "a connection task panicked");
    cancel.cancel();
}

#[nest_rs_core::main]
async fn read_the_flaky_feed() -> String {
    let Serving { port, cancel, .. } = serve_module::<FlakyFeedModule>(HttpConfig::default()).await;
    let mut flaky = request(port, "/flaky/events").await;
    let head = tokio::time::timeout(PATIENCE, read_head(&mut flaky))
        .await
        .expect("the flaky stream opens");
    let rest = read_to_end(&mut flaky).await;
    cancel.cancel();
    head + &rest
}

/// The body contains its stream's unwind under the process hook
/// `#[nest_rs::main]` installs: its one event says where the stream panicked,
/// and the hook stays quiet.
#[test]
fn a_stream_that_panics_is_said_once_where_it_panicked() {
    // `install` would see nothing: the server answers on the worker threads.
    let logs = LogCapture::install_global();

    let served = read_the_flaky_feed();
    // Rust's own hook back, so an assertion failing below prints.
    drop(std::panic::take_hook());

    assert!(served.contains("data: one"), "{served:?}");
    logs.expect_none(
        nest_rs_core::target::APP,
        "panicked where no unit of work contains it",
    );
    let said = logs.expect_one(nest_rs_http::target::HTTP, ENDED);
    assert_eq!(
        said.field(nest_rs_core::panic::FIELD).as_deref(),
        Some("the stream panicked")
    );
    let location = said
        .field(nest_rs_core::panic::LOCATION_FIELD)
        .unwrap_or_default();
    assert!(
        location.contains("tests/integration/sse.rs:"),
        "where it panicked: {said:#?}"
    );
}

/// Over HTTP/2 a stream that unwinds resets its own stream, and the connection
/// opens the next one.
#[tokio::test]
async fn a_stream_that_panics_over_http2_resets_its_stream_alone() {
    let logs = LogCapture::install();
    let Serving { port, cancel, .. } = serve_module::<FlakyFeedModule>(HttpConfig::default()).await;
    let (client, connection) = h2::client::handshake(connect(port).await)
        .await
        .expect("the HTTP/2 preface is accepted");
    let connection = tokio::spawn(connection);
    let get = |path: &str| {
        http::Request::get(format!("http://127.0.0.1:{port}{path}"))
            .body(())
            .expect("a request")
    };

    let mut ready = client.clone().ready().await.expect("a stream can open");
    let (answer, _) = ready
        .send_request(get("/flaky/events"), true)
        .expect("sent");
    let response = tokio::time::timeout(PATIENCE, answer)
        .await
        .expect("the head arrives")
        .expect("the stream opens");
    assert_eq!(response.status(), 200);
    let mut body = response.into_body();
    let mut served = Vec::new();
    let reset = loop {
        match tokio::time::timeout(PATIENCE, body.data())
            .await
            .expect("the stream ends")
        {
            Some(Ok(chunk)) => served.extend_from_slice(&chunk),
            Some(Err(reset)) => break reset,
            None => panic!("finished as a stream that ended: {served:?}"),
        }
    };
    let served = String::from_utf8_lossy(&served);
    assert!(
        served.contains("data: one"),
        "the event before it: {served:?}"
    );
    assert_eq!(reset.reason(), Some(h2::Reason::INTERNAL_ERROR), "{reset}");

    let mut ready = client.clone().ready().await.expect("a stream can open");
    let (answer, _) = ready.send_request(get("/flaky/ping"), true).expect("sent");
    let response = tokio::time::timeout(PATIENCE, answer)
        .await
        .expect("the stream ends")
        .expect("answered on the same connection");
    assert_eq!(response.status(), 200);
    assert!(!connection.is_finished(), "the connection keeps serving");
    logs.expect_one(nest_rs_http::target::HTTP, ENDED);
    logs.expect_none(nest_rs_http::target::HTTP, "an HTTP/2 stream task panicked");
    cancel.cancel();
}

/// Who a layer said the request is for, read back by the stream it opens.
#[derive(Debug)]
struct Caller(&'static str);

impl AmbientValue for Caller {}

/// Adds the caller around the handler, as a package's interceptor would.
#[injectable]
#[derive(Default)]
struct CallerInterceptor;

impl Layer for CallerInterceptor {}

#[async_trait]
impl Interceptor for CallerInterceptor {
    async fn intercept(&self, req: poem::Request, next: Next<'_>) -> poem::Result<poem::Response> {
        with_ambient(Arc::new(Caller("alice")), next.run(req)).await
    }
}

#[controller(path = "/caller")]
struct CallerFeedController;

#[routes]
impl CallerFeedController {
    /// Three events, each naming the caller the stream reads off the ambient
    /// context when the body is polled, long after the handler returned.
    #[sse("/events")]
    #[use_interceptors(CallerInterceptor)]
    async fn events(&self) -> SseStream {
        SseStream::new(stream::iter(1..=3).map(|n| {
            let caller = ambient::<Caller>().map_or("nobody", |caller| caller.0);
            SseEvent::message(format!("{n} {caller}"))
        }))
    }
}

#[module(providers = [CallerInterceptor, CallerFeedController])]
struct CallerFeedModule;

#[tokio::test]
async fn every_event_of_a_stream_reads_what_a_layer_added_for_its_request() {
    let client = boot::<CallerFeedModule>().await;
    let resp = client.get("/caller/events").send().await;
    resp.assert_status_is_ok();
    let text = resp
        .0
        .into_body()
        .into_string()
        .await
        .expect("the stream is text");
    for n in 1..=3 {
        assert!(
            text.contains(&format!("data: {n} alice")),
            "event {n} reads the caller: {text:?}",
        );
    }
}

/// One per request: the instance a stream resolves says whose scope it reads.
#[injectable(scope = request)]
#[derive(Default)]
struct Visit;

#[controller(path = "/visit")]
struct VisitFeedController;

#[routes]
impl VisitFeedController {
    /// Three events, each resolving the request's `Visit` when the body is
    /// polled and saying whether it is the instance the handler was given.
    #[sse("/events")]
    #[public]
    async fn events(&self, visit: Scoped<Visit>) -> SseStream {
        let given = visit.into_inner();
        SseStream::new(stream::iter(1..=3).map(move |n| {
            let resolved =
                nest_rs_http::current_request_scope().and_then(|scope| scope.get::<Visit>());
            let which = match resolved {
                Some(resolved) if Arc::ptr_eq(&resolved, &given) => "the request's",
                Some(_) => "another",
                None => "none",
            };
            SseEvent::message(format!("{n} {which}"))
        }))
    }
}

#[module(providers = [Visit, VisitFeedController])]
struct VisitFeedModule;

#[tokio::test]
async fn every_event_of_a_stream_resolves_its_requests_own_scoped_provider() {
    let client = boot::<VisitFeedModule>().await;
    let resp = client.get("/visit/events").send().await;
    resp.assert_status_is_ok();
    let text = resp
        .0
        .into_body()
        .into_string()
        .await
        .expect("the stream is text");
    for n in 1..=3 {
        assert!(
            text.contains(&format!("data: {n} the request's")),
            "event {n} resolves from the scope its handler ran in: {text:?}",
        );
    }
}
