//! `#[sse]` — the `text/event-stream` route, its ceiling and its refusals.
//!
//! The compile-time refusals (`#[authorize]`, a response decorator, a declared
//! `response_content_type`) are trybuild snapshots next door; what is asserted
//! here is what only a running route can show: the media type on the wire, the
//! frames a client actually reads, and the ceiling ending a stream that would
//! otherwise never stop.

use std::net::TcpListener as StdTcpListener;
use std::time::Duration;

use nest_rs_core::{App, Transport, module};
use nest_rs_http::{
    HttpConfig, HttpModule, HttpTransport, SseEvent, SseStream, controller, futures_util::stream,
    routes,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpSocket, TcpStream};
use tokio_util::sync::CancellationToken;

use crate::boot;

#[controller(path = "/feed")]
struct FeedController;

#[routes]
impl FeedController {
    /// Three events, then the stream ends on its own — the ordinary case.
    #[sse("/ticks")]
    #[public]
    async fn ticks(&self) -> SseStream {
        SseStream::new(stream::iter([
            SseEvent::message("one").event_type("tick").id("1"),
            SseEvent::message("two").event_type("tick").id("2"),
            SseEvent::message("three").event_type("tick").id("3"),
        ]))
    }

    /// A stream that never ends. Without the ceiling this request never
    /// completes, which is precisely the shape the ceiling exists for: the test
    /// below finishes only because the stream is closed for it.
    #[sse("/forever")]
    #[public]
    async fn forever(&self) -> SseStream {
        SseStream::new(stream::pending())
    }
}

/// A one-second ceiling, so the endless stream is closed inside the test rather
/// than four hours later. Pinned on the module, which is how the transport under
/// test learns it — the same seam a deployment uses.
#[module(imports = [HttpModule::for_root(
    HttpConfig { sse_max_connection: Some(Duration::from_secs(1)), ..HttpConfig::default() },
)], providers = [FeedController])]
struct FeedModule;

#[tokio::test]
async fn an_sse_route_answers_text_event_stream() {
    let client = boot::<FeedModule>().await;
    let resp = client.get("/feed/ticks").send().await;
    resp.assert_status_is_ok();
    // `assert_header` would compare against the exact string; poem appends the
    // charset, so the assertion is on the media type it starts with.
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
async fn the_events_reach_the_client_with_their_type_and_id() {
    let client = boot::<FeedModule>().await;
    let body = client.get("/feed/ticks").send().await.0.into_body();
    let text = body.into_string().await.expect("the stream is text");
    // The wire framing is the protocol's, not ours — asserting on it is what
    // proves the decorator emitted a real `text/event-stream` rather than a
    // body that merely carries the right header.
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

#[tokio::test]
async fn the_connection_ceiling_closes_a_stream_that_never_ends() {
    let client = boot::<FeedModule>().await;
    // The whole assertion is that this returns at all. `stream::pending()`
    // yields nothing, ever; only `NESTRS_HTTP__SSE_MAX_CONNECTION_SECS` ends it.
    // The timeout is the failure mode made explicit — without the ceiling the
    // await below would hang until the harness killed the run.
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

// ---- The residual, witnessed rather than claimed closed ---------------------

/// A client's receive buffer, pinned small — which is what makes "the peer
/// stopped reading" mean it. Setting `SO_RCVBUF` turns off Linux's window
/// autotuning, so the kernel stops absorbing megabytes on behalf of an
/// application that never reads and the server's write parks for real.
const CLIENT_RECV_BUFFER: u32 = 2048;

/// The ceiling the streaming route below is mounted with.
const CEILING: Duration = Duration::from_secs(1);

#[controller(path = "/flood")]
struct FloodController;

#[routes]
impl FloodController {
    /// Emits as fast as it is polled, in chunks big enough to fill a socket the
    /// peer is not draining — which is what parks the write.
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

/// The ceiling bounds **emission**, and this is what it does not bound. A peer
/// that stops reading parks the write, hyper stops polling the body, and the
/// socket — its task, its buffers, everything the stream holds — outlives the
/// ceiling until that peer reads again.
///
/// Asserted, not merely admitted, because two attempts to close it from inside
/// this crate were wrong in opposite directions and both looked right: bounding
/// the *socket* truncated unrelated responses under a declared `content-length`,
/// because a socket does not know whose bytes are queued. Until the transport
/// can express "this response is stalled", this test is what stops the gap from
/// being quietly reclassified as closed.
#[tokio::test]
async fn a_peer_that_stops_reading_still_holds_its_socket_past_the_ceiling() {
    let app = App::builder()
        .module::<FloodModule>()
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
        .write_all(
            b"GET /flood/events HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n",
        )
        .await
        .expect("the request is sent");
    socket.flush().await.expect("the request is flushed");

    // Well past the ceiling, without reading a byte.
    tokio::time::sleep(CEILING * 2).await;
    let mut buf = [0_u8; 1024];
    let still_there = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut buf)).await;
    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), serving).await;

    assert!(
        matches!(still_there, Ok(Ok(n)) if n > 0),
        "the socket still has the stream's buffered bytes to hand over long after \
         the ceiling — if this ever starts failing, the transport grew a control \
         this crate could not build, and the module docs saying so are stale",
    );
}

/// Loopback on a socket whose receive buffer is pinned to
/// [`CLIENT_RECV_BUFFER`], retrying while the listener comes up — `serve` binds
/// on its own task, so the first connect can lose the race.
async fn connect(port: u16) -> TcpStream {
    let addr = format!("127.0.0.1:{port}")
        .parse()
        .expect("loopback address");
    for _ in 0..100 {
        let socket = TcpSocket::new_v4().expect("a client socket");
        socket
            .set_recv_buffer_size(CLIENT_RECV_BUFFER)
            .expect("the receive window is pinned");
        if let Ok(socket) = socket.connect(addr).await {
            return socket;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the transport never came up on port {port}");
}
