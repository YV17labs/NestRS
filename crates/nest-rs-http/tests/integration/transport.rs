//! `HttpTransport::serve` once shutdown is asked for: the window poem is handed,
//! what it closes at the bound, and what it leaves open.
//!
//! Over real sockets, since the window governs connections and `TestClient` has
//! none. The clock is paused once the connections are up, so the default window
//! is proved at its full length without the suite waiting it out.

use std::net::TcpListener as StdTcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use nest_rs_core::{App, Transport, module};
use nest_rs_http::{HttpConfig, HttpTransport, SseStream, controller, routes};
use nest_rs_testing::LogCapture;
use poem::web::websocket::{Message as Frame, WebSocket};
use poem::{IntoResponse, handler};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

/// The line the transport files when the window closes on open connections.
const CUT: &str = "connections still open as the shutdown window closes are cut; a request still \
                   running is dropped unanswered and a stream ends mid-flow";

/// How long the slow route takes to answer — well inside the default window.
const SLOW: Duration = Duration::from_secs(2);

/// Real-time patience for a socket the transport has already answered or closed.
const PATIENCE: Duration = Duration::from_secs(5);

/// Told when the slow route has started, so shutdown is asked for while it runs.
static STARTED: Notify = Notify::const_new();

/// Told when the stuck route has started.
static STUCK: Notify = Notify::const_new();

/// Set when the stuck route's future is dropped — which is what cancelling it
/// is.
static CANCELLED: AtomicBool = AtomicBool::new(false);

struct SetOnDrop;

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        CANCELLED.store(true, Ordering::SeqCst);
    }
}

#[controller(path = "/shutdown")]
struct ShutdownController;

#[routes]
impl ShutdownController {
    /// A stream with nothing to say, ever: only the window ends it.
    #[sse("/stream")]
    #[public]
    async fn stream(&self) -> SseStream {
        SseStream::new(futures_util::stream::pending())
    }

    #[get("/slow")]
    #[public]
    async fn slow(&self) -> &'static str {
        STARTED.notify_one();
        tokio::time::sleep(SLOW).await;
        "answered"
    }

    #[get("/quick")]
    #[public]
    async fn quick(&self) -> &'static str {
        "quick"
    }

    /// Waits on something that never comes, past the request timeout's reach:
    /// the window closes first.
    #[get("/stuck")]
    #[public]
    async fn stuck(&self) -> &'static str {
        let _cancelled = SetOnDrop;
        STUCK.notify_one();
        std::future::pending::<()>().await;
        "never"
    }
}

#[module(providers = [ShutdownController])]
struct ShutdownModule;

/// A WebSocket that echoes text until its peer goes: the connection poem hands
/// to a handler at the upgrade and stops tracking.
#[handler]
fn echo(ws: WebSocket) -> impl IntoResponse {
    ws.on_upgrade(|mut socket| async move {
        while let Some(Ok(frame)) = socket.next().await {
            if let Frame::Text(text) = frame
                && socket.send(Frame::Text(text)).await.is_err()
            {
                break;
            }
        }
    })
}

/// A transport serving [`ShutdownModule`] and the echo socket on a local port,
/// built from the default [`HttpConfig`] — the same path `HttpModule` takes, so
/// the window under test is the one a deployment gets.
struct Serving {
    port: u16,
    cancel: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

async fn serve() -> Serving {
    let app = App::builder()
        .module::<ShutdownModule>()
        .build()
        .await
        .expect("module boots");
    let port = free_port();
    let mut transport = HttpTransport::from_config(&HttpConfig {
        host: "127.0.0.1".into(),
        port,
        ..HttpConfig::default()
    })
    .expect("the default config builds a transport")
    .mount("/socket", |_| poem::get(echo));
    transport
        .configure(app.container())
        .await
        .expect("transport configures");
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let cancel = cancel.clone();
        async move { Box::new(transport).serve(cancel).await }
    });
    Serving { port, cancel, task }
}

impl Serving {
    /// Ask for shutdown with the clock paused, and return how long `serve`
    /// took to come back on that clock.
    async fn stop(self) -> Duration {
        tokio::time::pause();
        let asked = Instant::now();
        self.cancel.cancel();
        self.task
            .await
            .expect("serve does not panic")
            .expect("serve stops cleanly");
        let took = asked.elapsed();
        tokio::time::resume();
        took
    }
}

fn free_port() -> u16 {
    let listener = StdTcpListener::bind(("127.0.0.1", 0)).expect("bind an ephemeral port");
    listener.local_addr().expect("the bound port").port()
}

/// Loopback, retrying while the listener comes up — `serve` binds on its own
/// task, so the first connect can lose the race.
async fn connect(port: u16) -> TcpStream {
    for _ in 0..100 {
        if let Ok(stream) = TcpStream::connect(("127.0.0.1", port)).await {
            return stream;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the transport never came up on port {port}");
}

/// Send `GET path` on a fresh connection.
async fn request(port: u16, path: &str) -> TcpStream {
    let mut stream = connect(port).await;
    stream
        .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
        .await
        .expect("the request is sent");
    stream
}

/// Read a response head, byte by byte, up to the blank line that ends it.
async fn read_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut byte).await.expect("the head is readable");
        assert!(
            read > 0,
            "the connection ended inside the head: {:?}",
            String::from_utf8_lossy(&head),
        );
        head.push(byte[0]);
    }
    String::from_utf8(head).expect("the head is text")
}

/// Everything left on the socket, up to its end — which only comes once the
/// server has closed it.
async fn read_to_end(stream: &mut TcpStream) -> String {
    let mut rest = Vec::new();
    match tokio::time::timeout(PATIENCE, stream.read_to_end(&mut rest)).await {
        Ok(Ok(_)) => {}
        // A reset ends the connection as surely as a close does.
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        Ok(Err(error)) => panic!("the socket failed rather than ending: {error}"),
        Err(_) => panic!("the server never closed the connection"),
    }
    String::from_utf8_lossy(&rest).into_owned()
}

/// An endless `#[sse]` stream would hold a graceful shutdown forever, and a
/// WebSocket is not poem's to wait for at all. The window bounds the first and
/// cuts it; the second is left open and said to be, and its handler still
/// answers after the transport has stopped.
#[tokio::test]
async fn a_stream_and_a_websocket_held_open_hold_the_shutdown_no_longer_than_its_window() {
    let logs = LogCapture::install();
    let serving = serve().await;
    let mut stream = request(serving.port, "/shutdown/stream").await;
    let head = read_head(&mut stream).await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "the stream opened: {head}"
    );
    let (mut websocket, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/socket", serving.port))
            .await
            .expect("the upgrade succeeds");

    let took = serving.stop().await;

    let window = HttpConfig::default().shutdown_timeout;
    assert!(
        took >= window,
        "shutdown waited on the open stream until the window closed, took {took:?}",
    );
    assert!(
        took < window + Duration::from_secs(1),
        "and not past it, took {took:?}",
    );
    let cut = logs.expect_one(nest_rs_http::target::HTTP, CUT);
    assert_eq!(cut.level, "warn");
    assert_eq!(
        cut.field("cut").as_deref(),
        Some("1"),
        "the stream is the one connection the window closed",
    );
    assert_eq!(
        cut.field("upgraded_open").as_deref(),
        Some("1"),
        "the WebSocket is still open, and the line says so",
    );
    assert_eq!(
        cut.field("shutdown_timeout_ms"),
        Some(window.as_millis().to_string()),
    );

    read_to_end(&mut stream).await;

    websocket
        .send(Message::text("still here"))
        .await
        .expect("the socket still takes a frame");
    let echoed = tokio::time::timeout(PATIENCE, websocket.next())
        .await
        .expect("the handler still answers")
        .expect("a frame")
        .expect("a frame, not an error");
    assert_eq!(echoed, Message::text("still here"));
}

/// A request already running when shutdown is asked for finishes inside the
/// window: it is answered, its connection is closed after the answer, and
/// shutdown comes back as soon as it is — not when the window closes.
#[tokio::test]
async fn a_request_in_flight_when_shutdown_starts_is_answered_inside_the_window() {
    let logs = LogCapture::install();
    let serving = serve().await;
    let mut client = request(serving.port, "/shutdown/slow").await;
    STARTED.notified().await;

    let took = serving.stop().await;

    // The handler's timer was set before the clock paused and rounds up to the
    // next millisecond, so "once it answered" is `SLOW`, give or take one tick —
    // and a world away from the window.
    assert!(
        took < SLOW + Duration::from_millis(100),
        "shutdown came back once the request was answered, took {took:?}",
    );
    let head = read_head(&mut client).await;
    assert!(head.starts_with("HTTP/1.1 200"), "answered: {head}");
    assert_eq!(read_to_end(&mut client).await, "answered");
    logs.expect_none(nest_rs_http::target::HTTP, CUT);
}

/// A request still running when the window closes is cut: its client gets no
/// answer, its handler is dropped where it waits — over HTTP/1.1, hyper polls
/// the handler inside the connection poem drops — and the line counts it.
#[tokio::test]
async fn a_request_still_running_when_the_window_closes_is_cut_unanswered() {
    let logs = LogCapture::install();
    let serving = serve().await;
    let mut client = request(serving.port, "/shutdown/stuck").await;
    STUCK.notified().await;

    let took = serving.stop().await;

    let window = HttpConfig::default().shutdown_timeout;
    assert!(
        took >= window && took < window + Duration::from_secs(1),
        "shutdown waited on the running request until the window closed, took {took:?}",
    );
    assert_eq!(
        read_to_end(&mut client).await,
        "",
        "the client got no answer, not a byte of one",
    );
    assert!(
        CANCELLED.load(Ordering::SeqCst),
        "the handler was dropped where it waited",
    );
    let cut = logs.expect_one(nest_rs_http::target::HTTP, CUT);
    assert_eq!(cut.field("cut").as_deref(), Some("1"));
    assert_eq!(cut.field("upgraded_open").as_deref(), Some("0"));
}

/// An idle kept-alive connection has nothing in flight, so it is closed at the
/// signal and the window is not spent on it.
#[tokio::test]
async fn an_idle_connection_is_closed_at_the_signal_without_spending_the_window() {
    let logs = LogCapture::install();
    let serving = serve().await;
    let mut client = request(serving.port, "/shutdown/quick").await;
    let head = read_head(&mut client).await;
    assert!(head.starts_with("HTTP/1.1 200"), "answered: {head}");
    let mut body = [0_u8; 5];
    client
        .read_exact(&mut body)
        .await
        .expect("the body is readable");
    assert_eq!(&body, b"quick");

    let took = serving.stop().await;

    assert!(
        took < Duration::from_secs(1),
        "nothing was in flight, so nothing was waited for — took {took:?}",
    );
    assert_eq!(read_to_end(&mut client).await, "");
    logs.expect_none(nest_rs_http::target::HTTP, CUT);
}
