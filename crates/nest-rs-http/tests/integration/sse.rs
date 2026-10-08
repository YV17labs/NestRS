//! `#[sse]` on a running route: the media type, the frames, and the ceiling. Its
//! compile-time refusals are trybuild snapshots in `nest-rs-macro-hygiene`.

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

/// A client's receive buffer, pinned small: setting `SO_RCVBUF` turns off Linux's
/// window autotuning, so the server's write parks for real.
const CLIENT_RECV_BUFFER: u32 = 2048;

/// The ceiling the streaming route below is mounted with.
const CEILING: Duration = Duration::from_secs(1);

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

/// The known gap: the ceiling bounds emission, not the socket. A peer that stops
/// reading parks the write, and the socket outlives the ceiling.
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

    // Paused time: nothing waits meanwhile but the parked write.
    tokio::time::pause();
    tokio::time::sleep(CEILING * 2).await;
    tokio::time::resume();
    let mut buf = [0_u8; 1024];
    let still_there = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut buf)).await;
    cancel.cancel();
    // The parked write holds the shutdown too.
    tokio::time::pause();
    let _ = tokio::time::timeout(Duration::from_secs(5), serving).await;
    tokio::time::resume();

    assert!(
        matches!(still_there, Ok(Ok(n)) if n > 0),
        "the socket still has the stream's buffered bytes to hand over long after \
         the ceiling — if this ever starts failing, the transport grew a control \
         this crate could not build, and the module docs saying so are stale",
    );
}

/// Loopback on a socket whose receive buffer is pinned to
/// [`CLIENT_RECV_BUFFER`], retrying while the listener comes up.
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
