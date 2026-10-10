//! `#[sse]` on a running route: the media type, the frames, the ceiling, and a
//! peer that stops reading. Its compile-time refusals are trybuild snapshots in
//! `nest-rs-macro-hygiene`.

use std::time::Duration;

use nest_rs_core::module;
use nest_rs_http::{
    HttpConfig, HttpModule, SseEvent, SseStream, controller, futures_util::stream, routes,
};
use nest_rs_testing::LogCapture;
use tokio::io::AsyncWriteExt;

use crate::boot;
use crate::transport::{PATIENCE, Serving, after, read_head, request, serve_module, small_window};

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
    async fn closed(&self) -> poem::Result<SseStream> {
        Err(poem::Error::from_status(
            poem::http::StatusCode::SERVICE_UNAVAILABLE,
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
type Opened = Result<SseStream, poem::Error>;

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
        .assert_status(poem::http::StatusCode::SERVICE_UNAVAILABLE);
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
