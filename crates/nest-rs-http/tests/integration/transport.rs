//! `HttpTransport::serve` once shutdown is asked for: what ends at the signal,
//! the window poem is handed, what it closes at the bound, and what it leaves
//! open.
//!
//! Over real sockets, since the window governs connections and `TestClient` has
//! none. The clock is paused once the connections are up, so the default window
//! is proved at its full length without the suite waiting it out.

use std::net::TcpListener as StdTcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use nest_rs_core::{App, ContainerBuilder, Transport, module};
use nest_rs_http::{
    DetachedWork, HttpConfig, HttpEndpointMeta, HttpTransport, SseStream, controller, routes,
};
use nest_rs_testing::LogCapture;
use poem::web::websocket::{Message as Frame, WebSocket};
use poem::{Body, IntoResponse, Route, handler};
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

    /// A body with an end of its own that it never reaches: a download, not an
    /// event stream, so only the window ends it.
    #[get("/download")]
    #[public]
    async fn download(&self) -> Body {
        Body::from_bytes_stream(
            futures_util::stream::once(async { Ok::<_, std::io::Error>(&b"part"[..]) })
                .chain(futures_util::stream::pending()),
        )
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
    serve_module::<ShutdownModule>().await
}

/// [`serve`] for any module — the same transport, the same echo socket.
async fn serve_module<M: nest_rs_core::Module + 'static>() -> Serving {
    let app = App::builder()
        .module::<M>()
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

/// The one `http.request` line filed for `path`.
fn operation_line(logs: &LogCapture, path: &str) -> nest_rs_testing::CapturedEvent {
    let mut lines: Vec<_> = logs
        .find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_http::unit::REQUEST,
        )
        .into_iter()
        .filter(|line| line.field("path").as_deref() == Some(path))
        .collect();
    assert_eq!(lines.len(), 1, "one line for {path}, got {lines:?}");
    lines.remove(0)
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

/// An `#[sse]` stream has no end of its own, so waiting on it could only ever
/// spend the whole window: it is ended at the signal instead — cleanly, its last
/// chunk written, so the client's `EventSource` reads an end and reconnects (to a
/// replica still in the load balancer) rather than a cut — and its line says the
/// transport, not the stream, ended it.
#[tokio::test]
async fn an_event_stream_is_ended_at_the_signal_and_files_its_line_cancelled() {
    let logs = LogCapture::install();
    let serving = serve().await;
    let mut stream = request(serving.port, "/shutdown/stream").await;
    let head = read_head(&mut stream).await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "the stream opened: {head}"
    );

    let took = serving.stop().await;

    assert!(
        took < Duration::from_secs(1),
        "shutdown did not wait on a stream with no end of its own, took {took:?}",
    );
    let rest = read_to_end(&mut stream).await;
    assert!(
        rest.ends_with("0\r\n\r\n"),
        "the body ended with its last chunk rather than a cut: {rest:?}",
    );
    logs.expect_none(nest_rs_http::target::HTTP, CUT);
    let line = operation_line(&logs, "/shutdown/stream");
    assert_eq!(
        line.field("status").as_deref(),
        Some("200"),
        "its head was answered"
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "and the transport ended its body before the stream did",
    );
    assert_span_cancelled(&logs, "/shutdown/stream");
}

/// A body with an end of its own is a response still being answered, like a
/// request still running: it gets the window, and is cut at its close. Its head
/// was answered, so its line carries the head's `status` and the `bytes` written
/// before the cut — and `cancelled`, since it never reached its end.
#[tokio::test]
async fn a_streamed_download_holds_the_window_and_is_cut_at_its_close() {
    let logs = LogCapture::install();
    let serving = serve().await;
    let mut download = request(serving.port, "/shutdown/download").await;
    let head = read_head(&mut download).await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "the download started: {head}"
    );

    let took = serving.stop().await;

    let window = HttpConfig::default().shutdown_timeout;
    assert!(
        took >= window && took < window + Duration::from_secs(1),
        "the download held the window to its close, and not past it — took {took:?}",
    );
    let cut = logs.expect_one(nest_rs_http::target::HTTP, CUT);
    assert_eq!(cut.level, "warn");
    assert_eq!(cut.field("cut").as_deref(), Some("1"));
    assert_eq!(cut.field("upgraded_open").as_deref(), Some("0"));
    assert_eq!(
        cut.field("shutdown_timeout_ms"),
        Some(window.as_millis().to_string()),
    );
    read_to_end(&mut download).await;
    let line = operation_line(&logs, "/shutdown/download");
    assert_eq!(line.field("status").as_deref(), Some("200"));
    assert_eq!(
        line.field("bytes").as_deref(),
        Some("4"),
        "the one part written"
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "the window cut the body before its end",
    );
    assert_span_cancelled(&logs, "/shutdown/download");
}

/// The span of a request whose body the transport stopped says so in the
/// line's word, as a request cut before it answered does.
fn assert_span_cancelled(logs: &LogCapture, path: &str) {
    let span = logs
        .spans()
        .into_iter()
        .find(|span| {
            span.name == nest_rs_http::unit::REQUEST
                && span.field("url.path").as_deref() == Some(path)
        })
        .expect("the request's span");
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "{path}: {:?}",
        span.fields,
    );
    assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
}

/// A hand-mounted endpoint is the developer's own: the transport cannot see
/// inside it, so a socket it upgraded is neither waited for nor closed. It is
/// left open, the line that closes the transport says so, and its handler still
/// answers after the transport has stopped.
#[tokio::test]
async fn a_hand_mounted_websocket_is_left_open_and_said_to_be() {
    let logs = LogCapture::install();
    let serving = serve().await;
    // The listener is up once a plain connection lands.
    drop(connect(serving.port).await);
    let (mut websocket, _) =
        tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/socket", serving.port))
            .await
            .expect("the upgrade succeeds");

    let took = serving.stop().await;

    assert!(
        took < Duration::from_secs(1),
        "nothing the transport tracks was open, took {took:?}"
    );
    let open = logs.expect_one(
        nest_rs_http::target::HTTP,
        "HTTP transport stopped with upgraded connections still open; they end with their \
         handlers or with the process",
    );
    assert_eq!(open.field("upgraded_open").as_deref(), Some("1"));

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

    let line = operation_line(&logs, "/shutdown/stuck");
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
        "the request dropped at the window still files its line, and says it was stopped",
    );
    assert_eq!(line.field("status"), None, "nothing was answered");
    assert_eq!(line.field("bytes"), None, "nothing was written");
    // Measured on the wall clock, which the paused test clock does not move, so
    // only its presence is asserted here.
    line.field("duration_ms")
        .expect("the line says how long the request ran")
        .parse::<f64>()
        .expect("a number of milliseconds");

    // And its span exports like an answered one's — under the route the router
    // had matched before the handler ran — and failed, with the line's word.
    let span = logs
        .spans()
        .into_iter()
        .find(|span| {
            span.name == nest_rs_http::unit::REQUEST
                && span.field("url.path").as_deref() == Some("/shutdown/stuck")
        })
        .expect("the cut request's span");
    assert_eq!(
        span.field("otel.name").as_deref(),
        Some("GET /shutdown/stuck"),
        "{:?}",
        span.fields,
    );
    assert_eq!(span.field("http.route").as_deref(), Some("/shutdown/stuck"));
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
    );
    assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
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

/// A self-mount whose every request starts one unit of [`DetachedWork`] that
/// never unwinds: polled once, so it is running, then never again — the shape of
/// a unit that blocks its thread, which no stop can reach.
fn stuck_mount(path: &'static str, owner: &'static str) -> HttpEndpointMeta {
    let work = DetachedWork::new();
    let carried = work.clone();
    HttpEndpointMeta::new(path, "probe", move |_container, route: Route| {
        let work = carried.clone();
        route.at(
            path,
            poem::endpoint::make(move |_| {
                let work = work.clone();
                async move {
                    let mut unit = Box::pin(work.run(std::future::pending::<()>()));
                    assert!(futures_util::poll!(unit.as_mut()).is_pending());
                    std::mem::forget(unit);
                    "started"
                }
            }),
        )
    })
    .owned_by(owner)
    .exempt()
    .runs_detached(work)
}

struct SettleA;
struct SettleB;

impl nest_rs_core::Discoverable for SettleA {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<SettleA, HttpEndpointMeta>(stuck_mount("/settle-a", "SettleA"))
    }
}

impl nest_rs_core::Discoverable for SettleB {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<SettleB, HttpEndpointMeta>(stuck_mount("/settle-b", "SettleB"))
    }
}

#[module(providers = [SettleA, SettleB])]
struct SettleModule;

/// What every self-mount carries is waited for once, not once per mount. Each
/// stuck unit holds the window — it is work still running — and then the
/// transport stops them all and waits [`nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT`]
/// for them together: the sum a grace period is sized against counts one
/// settle, so `k` mounts paying one each would spend `k − 1` of them past it.
#[tokio::test]
async fn work_stopped_on_several_mounts_is_waited_for_once() {
    let logs = LogCapture::install();
    let serving = serve_module::<SettleModule>().await;
    for path in ["/settle-a", "/settle-b"] {
        let mut client = request(serving.port, path).await;
        let head = read_head(&mut client).await;
        assert!(head.starts_with("HTTP/1.1 200"), "{path} started: {head}");
    }

    let took = serving.stop().await;

    let window = HttpConfig::default().shutdown_timeout;
    let settle = nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT;
    assert!(
        took >= window + settle,
        "work still running holds the window, then the settle — took {took:?}",
    );
    assert!(
        took < window + settle + settle / 2,
        "one settle for every mount, not one per mount — took {took:?}",
    );
    let unwound = logs.find(
        nest_rs_http::target::HTTP,
        "stopped work did not unwind within its bound; it runs on through the shutdown hooks",
    );
    let mut paths: Vec<_> = unwound
        .iter()
        .map(|line| (line.field("path"), line.field("still_running")))
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            (Some("/settle-a".to_owned()), Some("1".to_owned())),
            (Some("/settle-b".to_owned()), Some("1".to_owned())),
        ],
        "each mount names what it left running",
    );
}
