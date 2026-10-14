//! `HttpTransport::serve` once shutdown is asked for, over real sockets since
//! `TestClient` has none; the clock is paused once the connections are up.

use std::net::TcpListener as StdTcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use nest_rs_core::{App, ContainerBuilder, Layer, Transport, injectable, module};
use nest_rs_http::{
    DetachedWork, HttpConfig, HttpEndpointMeta, HttpTransport, SseStream, async_trait, controller,
    routes,
};
use nest_rs_interceptors::{AppBuilderInterceptorsExt, Interceptor, Next, interceptor};
use nest_rs_testing::LogCapture;
use poem::web::websocket::{Message as Frame, WebSocket};
use poem::{Body, IntoResponse, Route, handler};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpSocket, TcpStream};
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
pub(crate) const PATIENCE: Duration = Duration::from_secs(5);

/// Told when the slow route has started, so shutdown is asked for while it runs.
static STARTED: Notify = Notify::const_new();

/// Told when the stuck route has started.
static STUCK: Notify = Notify::const_new();

/// Set when the stuck route's future is dropped.
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

    /// A body with an end of its own that it never reaches, like a download.
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

    /// Waits on something that never comes; the window closes first.
    #[get("/stuck")]
    #[public]
    async fn stuck(&self) -> &'static str {
        let _cancelled = SetOnDrop;
        STUCK.notify_one();
        std::future::pending::<()>().await;
        "never"
    }
}

#[module(providers = [ShutdownController, EchoSocket])]
struct ShutdownModule;

/// A WebSocket that echoes text until its peer goes, outliving the HTTP
/// connection that upgraded it.
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

/// The echo socket, self-mounted by hand: it upgrades without declaring the
/// socket as [`DetachedWork`].
pub(crate) struct EchoSocket;

impl nest_rs_core::Discoverable for EchoSocket {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<EchoSocket, HttpEndpointMeta>(
            HttpEndpointMeta::new(
                &crate::PUBLIC_MOUNT,
                "/socket",
                "echo",
                |_container, route: Route| route.at("/socket", poem::get(echo)),
            )
            .owned_by("EchoSocket"),
        )
    }
}

/// A transport serving a module on a local port, built from an [`HttpConfig`]
/// as `HttpModule` builds it.
pub(crate) struct Serving {
    pub(crate) port: u16,
    pub(crate) cancel: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

/// [`ShutdownModule`] from the default config.
async fn serve() -> Serving {
    serve_module::<ShutdownModule>(HttpConfig::default()).await
}

/// [`serve`] for any module and config — the same transport.
pub(crate) async fn serve_module<M: nest_rs_core::Module + 'static>(config: HttpConfig) -> Serving {
    serve_app(App::builder().module::<M>(), config).await
}

/// [`serve_module`] for an app built as `builder` describes.
async fn serve_app(builder: nest_rs_core::AppBuilder, config: HttpConfig) -> Serving {
    let app = builder.build().await.expect("module boots");
    let port = free_port();
    let mut transport = crate::built(&HttpConfig {
        host: "127.0.0.1".into(),
        port,
        ..config
    });
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
            nest_rs_http::unit::REQUEST.name(),
        )
        .into_iter()
        .filter(|line| line.field("path").as_deref() == Some(path))
        .collect();
    assert_eq!(lines.len(), 1, "one line for {path}, got {lines:?}");
    lines.remove(0)
}

pub(crate) fn free_port() -> u16 {
    let listener = StdTcpListener::bind(("127.0.0.1", 0)).expect("bind an ephemeral port");
    listener.local_addr().expect("the bound port").port()
}

/// Loopback, retrying while the listener comes up — `serve` binds on its own
/// task, so the first connect can lose the race.
pub(crate) async fn connect(port: u16) -> TcpStream {
    for _ in 0..100 {
        if let Ok(stream) = TcpStream::connect(("127.0.0.1", port)).await {
            return stream;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the transport never came up on port {port}");
}

/// [`connect`] on a socket whose receive buffer is 4 KiB.
pub(crate) async fn small_window(port: u16) -> TcpStream {
    for _ in 0..100 {
        let socket = TcpSocket::new_v4().expect("a socket");
        socket
            .set_recv_buffer_size(4 * 1024)
            .expect("a small receive buffer");
        if let Ok(stream) = socket.connect(([127, 0, 0, 1], port).into()).await {
            return stream;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the transport never came up on port {port}");
}

/// Send `GET path` on a connection already open.
pub(crate) async fn ask(stream: &mut TcpStream, path: &str) {
    stream
        .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
        .await
        .expect("the request is sent");
}

/// Send `GET path` on a fresh connection.
pub(crate) async fn request(port: u16, path: &str) -> TcpStream {
    let mut stream = connect(port).await;
    ask(&mut stream, path).await;
    stream
}

/// Read a response head, byte by byte, up to the blank line that ends it.
pub(crate) async fn read_head(stream: &mut (impl AsyncRead + Unpin)) -> String {
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

/// One answer off a kept-alive connection, read whole: its head, and the body
/// its `Content-Length` sizes.
pub(crate) async fn read_answer(stream: &mut TcpStream) -> (String, String) {
    let head = tokio::time::timeout(PATIENCE, read_head(stream))
        .await
        .expect("answered");
    let length: usize = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|value| value.trim().parse().expect("a length"))
        })
        .expect("a sized body");
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).await.expect("the body");
    (head, String::from_utf8(body).expect("text"))
}

/// Let `span` pass on the paused clock — every deadline the server armed
/// inside it fires — then hand the clock back. The socket is then read on the
/// real clock: a loopback byte can land after the paused one has moved on.
pub(crate) async fn after(span: Duration) {
    tokio::time::pause();
    tokio::time::sleep(span).await;
    tokio::time::resume();
}

/// Whether the server still holds `stream` open: no end is read within a
/// tenth of a second.
pub(crate) async fn still_open(stream: &mut TcpStream) -> bool {
    let mut byte = [0_u8; 1];
    tokio::time::timeout(Duration::from_millis(100), stream.read(&mut byte))
        .await
        .is_err()
}

/// Everything left on the socket, up to the server's close.
pub(crate) async fn read_to_end(stream: &mut TcpStream) -> String {
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

/// An `#[sse]` stream is ended cleanly at the signal, and its line files it
/// `cancelled`.
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

/// A body with an end of its own gets the window and is cut at its close; its
/// line carries the head's `status`, the `bytes` written and `cancelled`.
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
            span.name == nest_rs_http::unit::REQUEST.name()
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

/// A socket a hand-built self-mount upgraded without declaring it is neither
/// waited for nor closed: it outlives the transport, and the closing line says
/// so.
#[tokio::test]
async fn an_undeclared_upgraded_socket_is_left_open_and_said_to_be() {
    let logs = LogCapture::install();
    let serving = serve().await;
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

/// A request already running when shutdown is asked for is answered inside the
/// window, and shutdown returns as soon as it is.
#[tokio::test]
async fn a_request_in_flight_when_shutdown_starts_is_answered_inside_the_window() {
    let logs = LogCapture::install();
    let serving = serve().await;
    let mut client = request(serving.port, "/shutdown/slow").await;
    STARTED.notified().await;

    let took = serving.stop().await;

    // The handler's timer was set before the clock paused and rounds up to the
    // next millisecond, so this is `SLOW` give or take one tick.
    assert!(
        took < SLOW + Duration::from_millis(100),
        "shutdown came back once the request was answered, took {took:?}",
    );
    let head = read_head(&mut client).await;
    assert!(head.starts_with("HTTP/1.1 200"), "answered: {head}");
    assert_eq!(read_to_end(&mut client).await, "answered");
    logs.expect_none(nest_rs_http::target::HTTP, CUT);
}

/// A request with no budget of its own still running when the window closes is
/// cut: no answer, its handler dropped where it waits, and the line counts it.
#[tokio::test]
async fn a_request_still_running_when_the_window_closes_is_cut_unanswered() {
    let logs = LogCapture::install();
    let serving = serve_module::<ShutdownModule>(HttpConfig {
        request_timeout: None,
        ..HttpConfig::default()
    })
    .await;
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
    // Measured on the wall clock, which the paused clock does not move.
    line.field("duration_ms")
        .expect("the line says how long the request ran")
        .parse::<f64>()
        .expect("a number of milliseconds");

    let span = logs
        .spans()
        .into_iter()
        .find(|span| {
            span.name == nest_rs_http::unit::REQUEST.name()
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

/// With the defaults a request's budget fits the window, so one admitted before
/// a stop is answered — `503` at its own deadline — rather than cut.
#[tokio::test]
async fn with_the_defaults_a_request_admitted_before_a_stop_is_answered_inside_the_window() {
    let logs = LogCapture::install();
    let serving = serve().await;
    let mut client = request(serving.port, "/shutdown/stuck").await;
    STUCK.notified().await;
    after(Duration::from_secs(5)).await;

    let took = serving.stop().await;

    let window = HttpConfig::default().shutdown_timeout;
    assert!(
        took < window,
        "the request's deadline came before the window's close, took {took:?}",
    );
    let answer = read_to_end(&mut client).await;
    assert!(answer.starts_with("HTTP/1.1 503"), "{answer}");
    logs.expect_none(nest_rs_http::target::HTTP, CUT);
}

/// An idle kept-alive connection is closed at the signal.
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
/// is polled once and never again, as a unit blocking its thread would be.
fn stuck_mount(path: &'static str, owner: &'static str) -> HttpEndpointMeta {
    let work = DetachedWork::new();
    let carried = work.clone();
    HttpEndpointMeta::new(
        &crate::PUBLIC_MOUNT,
        path,
        "probe",
        move |_container, route: Route| {
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
        },
    )
    .owned_by(owner)
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

/// Stuck units on several self-mounts hold the window, then cost one
/// [`nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT`] together, not one per mount.
#[tokio::test]
async fn work_stopped_on_several_mounts_is_waited_for_once() {
    let logs = LogCapture::install();
    let serving = serve_module::<SettleModule>(HttpConfig::default()).await;
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

/// What a deployment whose request budget outlasts its shutdown window is told
/// at boot.
const OUTLASTS: &str = "the request timeout outlasts the shutdown window: a request admitted \
                        just before a stop can be cut unanswered";

/// The transport [`HttpModule`](nest_rs_http::HttpModule) builds from the
/// environment `vars` over the defaults.
fn built_from(vars: &[(&str, &str)]) -> HttpTransport {
    use nest_rs_config::Config as _;
    let config = HttpConfig::from_env(
        &nest_rs_config::ConfigService::with_vars("http", vars.iter().copied()),
        HttpConfig::default(),
    )
    .expect("the environment is in range");
    HttpTransport::from_config(&config).expect("a deployment may accept it knowingly")
}

/// A budget past the window, or none at all, is said once where the config is
/// read; a warning, not a refusal.
#[test]
fn a_request_timeout_outlasting_the_shutdown_window_is_said_once_at_boot() {
    let window = HttpConfig::default().shutdown_timeout.as_secs().to_string();
    for (value, said) in [("30", "30"), ("0", "0")] {
        let logs = LogCapture::install();
        built_from(&[("REQUEST_TIMEOUT_SECS", value)]);
        let line = logs.expect_one(nest_rs_http::target::HTTP, OUTLASTS);
        assert_eq!(line.level, "warn", "{value}");
        assert_eq!(
            line.field("request_timeout_secs").as_deref(),
            Some(said),
            "{value}"
        );
        assert_eq!(
            line.field("shutdown_timeout_secs").as_deref(),
            Some(window.as_str()),
            "{value}"
        );
    }
}

/// The defaults are ordered, and a budget inside the window says nothing.
#[test]
fn a_request_timeout_inside_the_shutdown_window_is_not_remarked() {
    for vars in [&[][..], &[("REQUEST_TIMEOUT_SECS", "5")][..]] {
        let logs = LogCapture::install();
        built_from(vars);
        logs.expect_none(nest_rs_http::target::HTTP, OUTLASTS);
    }
}

/// What a deployment whose body-read gap reaches its request budget is told at
/// boot.
const GAP_REACHES_THE_BUDGET: &str = "the body-read timeout reaches the request timeout: a body \
                                      that stops arriving is answered the request timeout's \
                                      503, never 408";

/// A gap at or past the budget can never fire first; a warning, not a refusal.
#[test]
fn a_body_read_gap_reaching_the_request_timeout_is_said_once_at_boot() {
    for (gap, budget) in [("20", "20"), ("30", "20")] {
        let logs = LogCapture::install();
        built_from(&[
            ("BODY_READ_TIMEOUT_SECS", gap),
            ("REQUEST_TIMEOUT_SECS", budget),
        ]);
        let line = logs.expect_one(nest_rs_http::target::HTTP, GAP_REACHES_THE_BUDGET);
        assert_eq!(line.level, "warn", "{gap}");
        assert_eq!(line.field("body_read_timeout_secs").as_deref(), Some(gap));
        assert_eq!(line.field("request_timeout_secs").as_deref(), Some(budget));
    }
}

/// The defaults keep the gap under the budget, and with the budget off the gap
/// is the one bound on a body.
#[test]
fn a_body_read_gap_under_the_request_timeout_or_without_one_is_not_remarked() {
    for vars in [
        &[][..],
        &[("BODY_READ_TIMEOUT_SECS", "19")][..],
        &[
            ("BODY_READ_TIMEOUT_SECS", "60"),
            ("REQUEST_TIMEOUT_SECS", "0"),
        ][..],
    ] {
        let logs = LogCapture::install();
        built_from(vars);
        logs.expect_none(nest_rs_http::target::HTTP, GAP_REACHES_THE_BUDGET);
    }
}

/// An interceptor that only forwards: registered, it puts the bands between
/// the edge and the router.
#[injectable]
#[derive(Default)]
struct Forwarding;

impl Layer for Forwarding {}

#[async_trait]
impl Interceptor for Forwarding {
    async fn intercept(&self, req: poem::Request, next: Next<'_>) -> poem::Result<poem::Response> {
        next.run(req).await
    }
}

/// What both upgrading leaves answer: the switch to the `echo` protocol.
fn switching() -> nest_rs_http::Response {
    nest_rs_http::Response::builder()
        .status(nest_rs_http::StatusCode::SWITCHING_PROTOCOLS)
        .header(
            nest_rs_http::header::CONNECTION,
            nest_rs_http::HeaderValue::from_static("upgrade"),
        )
        .header(
            nest_rs_http::header::UPGRADE,
            nest_rs_http::HeaderValue::from_static("echo"),
        )
        .finish()
}

/// Echo the first four bytes the peer sends on the switched socket.
async fn echo_four(mut socket: impl AsyncRead + tokio::io::AsyncWrite + Unpin) {
    let mut bytes = [0_u8; 4];
    if socket.read_exact(&mut bytes).await.is_ok() {
        let _ = socket.write_all(&bytes).await;
    }
}

/// A poem leaf taking the socket through poem's own upgrade, as a WebSocket
/// gateway does until it leaves poem.
#[handler]
fn poem_side(req: &poem::Request) -> poem::Response {
    let Ok(upgrade) = req.take_upgrade() else {
        return poem::Response::builder()
            .status(poem::http::StatusCode::UPGRADE_REQUIRED)
            .finish();
    };
    tokio::spawn(async move {
        if let Ok(socket) = upgrade.await {
            echo_four(socket).await;
        }
    });
    nest_rs_http::__private::poem_bridge::response_to_poem(switching())
}

/// A leaf on nestrs's vocabulary taking the socket through hyper's upgrade
/// the request carries.
async fn nestrs_side(req: nest_rs_http::Request) -> nest_rs_http::Result<nest_rs_http::Response> {
    let Some(upgrade) = req.extensions().get::<hyper::upgrade::OnUpgrade>().cloned() else {
        return Ok(nest_rs_http::Response::builder()
            .status(nest_rs_http::StatusCode::UPGRADE_REQUIRED)
            .finish());
    };
    tokio::spawn(async move {
        if let Ok(socket) = upgrade.await {
            echo_four(hyper_util::rt::TokioIo::new(socket)).await;
        }
    });
    Ok(switching())
}

/// Both upgrading leaves, self-mounted.
struct Upgrades;

impl nest_rs_core::Discoverable for Upgrades {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<Upgrades, HttpEndpointMeta>(
            HttpEndpointMeta::new(
                &crate::PUBLIC_MOUNT,
                "/upgrade/poem",
                "upgrade",
                |_container, route: Route| {
                    route.at("/upgrade/poem", poem::get(poem_side)).at(
                        "/upgrade/nestrs",
                        poem::get(nest_rs_http::__private::poem_bridge::to_poem(
                            nest_rs_http::endpoint_fn(nestrs_side),
                        )),
                    )
                },
            )
            .also_mounts(["/upgrade/nestrs"])
            .owned_by("Upgrades"),
        )
    }
}

#[module(providers = [Upgrades, Forwarding])]
struct UpgradeModule;

/// An upgrade request crosses the edge, the bands when one is registered and the
/// router with its socket still reachable, and a leaf on either side of the
/// bridge switches it.
#[tokio::test]
async fn an_upgrade_request_keeps_its_on_upgrade_through_the_stack() {
    for banded in [false, true] {
        let builder = App::builder().module::<UpgradeModule>();
        let builder = if banded {
            builder.use_interceptors_global([interceptor::<Forwarding>()])
        } else {
            builder
        };
        let serving = serve_app(builder, HttpConfig::default()).await;
        for leaf in ["/upgrade/poem", "/upgrade/nestrs"] {
            let mut stream = connect(serving.port).await;
            stream
                .write_all(
                    format!(
                        "GET {leaf} HTTP/1.1\r\nhost: localhost\r\nconnection: upgrade\r\n\
                         upgrade: echo\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .expect("the upgrade is asked for");
            let head = tokio::time::timeout(PATIENCE, read_head(&mut stream))
                .await
                .expect("answered");
            assert!(
                head.starts_with("HTTP/1.1 101"),
                "{leaf}, banded {banded}: {head}"
            );
            stream.write_all(b"ping").await.expect("sent on the socket");
            let mut echoed = [0_u8; 4];
            tokio::time::timeout(PATIENCE, stream.read_exact(&mut echoed))
                .await
                .expect("echoed in time")
                .expect("echoed");
            assert_eq!(&echoed, b"ping", "{leaf}, banded {banded}");
        }
        serving.cancel.cancel();
    }
}
