//! Driving a WebSocket gateway over a **real** upgrade.
//!
//! Every other edge answers in process: HTTP, GraphQL, OpenAPI and MCP all ride
//! poem's `TestClient`, and even a graphql-ws subscription can be driven by
//! handing async-graphql's own protocol engine a stream of client messages (see
//! [`crate::graphql`]). WS is the one edge where that is not possible, because
//! the protocol *is* the socket: the upgrade, the [`WsConfig`] the upgrade
//! resolves, the socket-lifetime ceiling, the writer task, the registry entry's
//! unwind cleanup and the per-message request scope all live in the connection
//! task poem spawns from `on_upgrade`, and nothing above `Gateway::dispatch`
//! runs until a client has actually connected.
//!
//! So this driver binds a port. [`WsApp`] boots the app's own HTTP transport —
//! the one its `HttpModule::for_root(cfg)` describes, so the global prefix and
//! everything else match what ships — on a free local address, and
//! [`WsSocket`] speaks the gateway's `{ event, data }` envelope over it.
//!
//! ```ignore
//! let app = TestApp::builder().module::<ChatModule>().build_ws().await?;
//! let mut socket = app.socket("/ws").bearer(&token).connect().await;
//! socket.send("message", json!({ "text": "hi" })).await;
//! assert_eq!(socket.next_envelope().await["event"], "message");
//! app.shutdown().await?;
//! ```
//!
//! Close frames are read as well as messages: [`WsSocket::expect_close`]
//! returns the RFC 6455 §7.4.1 code the server ended the socket with, which is
//! how a suite tells a deliberate close (the lifetime ceiling) from the
//! **1006 Abnormal Closure** a dropped connection produces.
//!
//! [`WsConfig`]: https://docs.rs/nest-rs-ws

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures_util::{SinkExt, StreamExt};
use nest_rs_core::Container;
use nest_rs_http::HttpTransport;
use nest_rs_ws::WsEnvelope;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{Error as ClientError, Message as ClientMessage};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::headless::{HeadlessApp, TransportHandle};

// The WebSocket status codes RFC 6455 §7.4.1 defines, from `nest-rs-ws` — the
// crate that closes sockets with them, and the one path a suite names them
// through (`nest_rs::ws::CloseCode`). Imported rather than re-exported: a
// second nestrs-adjacent path to one wire constant is two authorities on it,
// which is the duplication moving the type to `nest-rs-ws` removed.
use nest_rs_ws::CloseCode;

/// How long [`WsSocket::next_frame`] waits before reporting silence. Long
/// enough that a loaded CI box does not flake, short enough that a test
/// asserting *absence* stays quick — [`crate::graphql`]'s budget, for the same
/// reason.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long [`WsSocketBuilder::connect`] keeps retrying the handshake. The
/// transport binds inside its own task, so the first attempt can land before
/// the listener exists.
const CONNECT_BUDGET: Duration = Duration::from_secs(5);

/// Between handshake attempts.
const CONNECT_BACKOFF: Duration = Duration::from_millis(20);

/// Reserve a free local address by binding one, reading it back, and letting it
/// go.
///
/// [`HttpTransport`] binds inside `serve`, so it cannot report the port the OS
/// gave it; asking for one here and handing it over is the way round that, and
/// the window between the release and the rebind is what
/// [`WsSocketBuilder::connect`]'s retry covers.
fn reserve_addr() -> Result<SocketAddr> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .context("no free local port for the test transport")?;
    let addr = listener.local_addr()?;
    drop(listener);
    Ok(addr)
}

pub(crate) async fn serve(app: HeadlessApp, transport: HttpTransport) -> Result<WsApp> {
    let addr = reserve_addr()?;
    // `spawn_transport` configures then serves — the order `TestAppBuilder::build`
    // uses, so `init` (health indicators, the social registry, every
    // `OnApplicationBootstrap` hook) still runs against a configured transport.
    let handle = app
        .spawn_transport(transport.bind(addr.to_string()))
        .await?;
    app.init().await?;
    Ok(WsApp {
        app,
        handle: Some(handle),
        addr,
    })
}

/// A booted app serving its HTTP surface on a real local port — what a WS
/// upgrade needs and `TestClient` cannot give.
pub struct WsApp {
    app: HeadlessApp,
    handle: Option<TransportHandle>,
    addr: SocketAddr,
}

impl WsApp {
    /// The DI [`Container`], for resolving providers directly in assertions —
    /// the gateway's own `WsServer`, above all, whose `connection_count` is how
    /// a test observes the registry from outside the connection.
    pub fn container(&self) -> &Container {
        self.app.container()
    }

    /// The address the transport is listening on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The `ws://` URL of a mounted gateway path.
    pub fn url(&self, path: &str) -> String {
        format!("ws://{}/{}", self.addr, path.trim_start_matches('/'))
    }

    /// Open a socket against a mounted gateway path.
    pub fn socket(&self, path: &str) -> WsSocketBuilder {
        WsSocketBuilder {
            url: self.url(path),
            headers: Vec::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Stop the transport and await its clean exit, surfacing any error it
    /// terminated with. Dropping the [`WsApp`] instead detaches the task.
    pub async fn shutdown(mut self) -> Result<()> {
        match self.handle.take() {
            Some(handle) => handle.shutdown().await,
            None => Ok(()),
        }
    }
}

/// Builds a [`WsSocket`]: the upgrade request's headers, and the read budget
/// the socket inherits.
pub struct WsSocketBuilder {
    url: String,
    headers: Vec<(String, String)>,
    timeout: Duration,
}

impl WsSocketBuilder {
    /// Set a header on the **upgrade request** — which is where a gateway's
    /// connection-level guards run, so this is how a socket is authenticated.
    #[must_use]
    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    /// `Authorization: Bearer <token>` on the upgrade.
    #[must_use]
    pub fn bearer(self, token: &str) -> Self {
        self.header("authorization", format!("Bearer {token}"))
    }

    /// How long the socket's reads wait before reporting silence.
    #[must_use]
    pub fn timeout(mut self, within: Duration) -> Self {
        self.timeout = within;
        self
    }

    /// Open the socket, panicking with the URL if the handshake never
    /// succeeds. Use [`try_connect`](Self::try_connect) to assert that an
    /// upgrade is *refused*.
    pub async fn connect(self) -> WsSocket {
        let url = self.url.clone();
        match self.try_connect().await {
            Ok(socket) => socket,
            Err(err) => panic!("could not open a websocket to {url}: {err:#}"),
        }
    }

    /// Open the socket, reporting a refused handshake as an error.
    ///
    /// A connection *refused* at the TCP level is retried — the transport may
    /// not have finished binding — while a handshake the server answered and
    /// declined is returned straight away: that answer is the assertion, and
    /// retrying it would only trade it for a timeout.
    pub async fn try_connect(self) -> Result<WsSocket> {
        let deadline = tokio::time::Instant::now() + CONNECT_BUDGET;
        loop {
            let mut request = self
                .url
                .as_str()
                .into_client_request()
                .with_context(|| format!("`{}` is not a websocket url", self.url))?;
            for (name, value) in &self.headers {
                request.headers_mut().insert(
                    poem::http::HeaderName::from_bytes(name.as_bytes())
                        .with_context(|| format!("`{name}` is not a header name"))?,
                    value
                        .parse()
                        .with_context(|| format!("`{value}` is not a header value"))?,
                );
            }
            match tokio_tungstenite::connect_async(request).await {
                Ok((stream, _)) => {
                    return Ok(WsSocket {
                        stream,
                        timeout: self.timeout,
                    });
                }
                Err(ClientError::Io(_)) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(CONNECT_BACKOFF).await;
                }
                Err(err) => return Err(anyhow::anyhow!(err)),
            }
        }
    }
}

/// One frame read off the socket, in the vocabulary a suite asserts on. `Ping`
/// and `Pong` never appear: the protocol layer answers them, so surfacing them
/// would only make every test filter them out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsFrame {
    /// A text frame — a gateway's replies and pushes are all of these.
    Text(String),
    /// A binary frame.
    Binary(Vec<u8>),
    /// The close handshake, with the §7.4.1 code and reason when the peer sent
    /// them. `None` is a Close frame carrying no status at all, which §7.4.1
    /// reads as 1005 and is *not* the same as no Close frame.
    Close(Option<(CloseCode, String)>),
}

/// What one read off the socket found — the three states `Option<WsFrame>`
/// collapses into `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsRead {
    /// A frame arrived.
    Frame(WsFrame),
    /// Nothing arrived within the budget, and the socket is still open.
    Silent,
    /// The socket ended with no Close frame — §7.4.1's **1006 Abnormal
    /// Closure** — carrying the transport error when there was one.
    Aborted(Option<String>),
}

/// One live WebSocket connection, driven frame by frame.
pub struct WsSocket {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
    timeout: Duration,
}

impl WsSocket {
    /// Send one `{ event, data }` envelope — the gateway's whole wire grammar.
    ///
    /// Encoded by [`WsEnvelope`], the gateway's own encoder, so the driver
    /// cannot frame an envelope the gateway would not.
    pub async fn send(&mut self, event: &str, data: Value) {
        let frame = WsEnvelope::encode(event, &data).expect("a JSON value re-encodes");
        self.send_text(frame).await;
    }

    /// Send a raw text frame, for a payload the envelope grammar would not let
    /// you express — a malformed envelope, or one past the configured cap.
    pub async fn send_text(&mut self, text: impl Into<String>) {
        self.stream
            .send(ClientMessage::Text(text.into().into()))
            .await
            .expect("the socket accepts a text frame");
    }

    /// Send a binary frame — RFC 6455 §5.6 data the gateway's text-envelope
    /// contract has to answer for.
    pub async fn send_binary(&mut self, bytes: Vec<u8>) {
        self.stream
            .send(ClientMessage::Binary(bytes.into()))
            .await
            .expect("the socket accepts a binary frame");
    }

    /// The next `{ event, data }` envelope. Panics on silence or on a socket
    /// the server closed first — both are the assertion failing somewhere less
    /// obvious than here.
    pub async fn next_envelope(&mut self) -> Value {
        match self.next_frame().await {
            Some(WsFrame::Text(text)) => {
                serde_json::from_str(&text).expect("a gateway frame is a JSON envelope")
            }
            Some(WsFrame::Binary(bytes)) => {
                panic!("expected an envelope, got {} binary bytes", bytes.len())
            }
            Some(WsFrame::Close(close)) => {
                panic!("the server closed the socket before replying: {close:?}")
            }
            None => panic!("the server sent nothing within {:?}", self.timeout),
        }
    }

    /// The next frame of any kind, `None` on silence within the socket's
    /// budget or once the stream has ended.
    pub async fn next_frame(&mut self) -> Option<WsFrame> {
        self.next_frame_within(self.timeout).await
    }

    /// [`next_frame`](Self::next_frame) with an explicit budget — use a short
    /// one when asserting that **nothing** arrives, so the test does not pay
    /// the full timeout to prove silence.
    ///
    /// `None` is **silence**, and nothing else. A socket that died without a
    /// Close frame is [`WsRead::Aborted`] through
    /// [`read_within`](Self::read_within) — see there for why the two must not
    /// share a value.
    pub async fn next_frame_within(&mut self, within: Duration) -> Option<WsFrame> {
        match self.read_within(within).await {
            WsRead::Frame(frame) => Some(frame),
            WsRead::Silent | WsRead::Aborted(_) => None,
        }
    }

    /// The next frame, distinguishing the three outcomes `Option` collapses.
    ///
    /// The distinction is the whole reason this driver binds a socket. RFC 6455
    /// §7.4.1 reserves **1006 Abnormal Closure** for "the connection was closed
    /// abnormally, e.g., without sending or receiving a Close frame" — a
    /// *different* state from an idle connection, and the one a gateway defect
    /// produces. Folded into one `None`, a socket that died mid-test satisfied
    /// [`expect_silence`](Self::expect_silence): the assertion "nothing was
    /// sent" passed because nothing *could* be sent.
    pub async fn read_within(&mut self, within: Duration) -> WsRead {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let Ok(next) = tokio::time::timeout(remaining, self.stream.next()).await else {
                return WsRead::Silent;
            };
            let Some(message) = next else {
                return WsRead::Aborted(None);
            };
            match message {
                Ok(ClientMessage::Text(text)) => {
                    return WsRead::Frame(WsFrame::Text(text.to_string()));
                }
                Ok(ClientMessage::Binary(bytes)) => {
                    return WsRead::Frame(WsFrame::Binary(bytes.into()));
                }
                Ok(ClientMessage::Close(frame)) => {
                    return WsRead::Frame(WsFrame::Close(frame.map(close)));
                }
                // Answered by the protocol layer; never an assertion's subject.
                Ok(_) => continue,
                // No Close frame — §7.4.1's 1006. Also where tungstenite reports
                // a peer that violated framing, which is a gateway defect and
                // never a quiet connection.
                Err(err) => return WsRead::Aborted(Some(err.to_string())),
            }
        }
    }

    /// Assert nothing reaches the client within `within`.
    ///
    /// Fails on an aborted socket as loudly as on an unexpected frame: a
    /// connection that died proves nothing about what the gateway would have
    /// sent.
    pub async fn expect_silence(&mut self, within: Duration) {
        match self.read_within(within).await {
            WsRead::Silent => {}
            WsRead::Frame(frame) => panic!("expected silence, got {frame:?}"),
            WsRead::Aborted(err) => panic!(
                "expected silence, but the socket died without a Close frame \
                 (§7.4.1 reads that as 1006 Abnormal Closure): {err:?}"
            ),
        }
    }

    /// Read until the server's Close frame and return the §7.4.1 code and
    /// reason it carried.
    ///
    /// Panics when the socket ends without one, because that is exactly the
    /// defect worth failing on: a peer that reads **1006** cannot tell a
    /// deliberate close from a network fault, so "the connection went away" is
    /// never an acceptable pass.
    pub async fn expect_close(&mut self) -> (CloseCode, String) {
        loop {
            match self.read_within(self.timeout).await {
                WsRead::Frame(WsFrame::Close(Some(close))) => return close,
                WsRead::Frame(WsFrame::Close(None)) => {
                    panic!("the server closed with no status code at all (§7.4.1 reads that 1005)")
                }
                WsRead::Frame(_) => {}
                WsRead::Aborted(err) => panic!(
                    "the socket ended with no Close frame — the peer reads that as 1006 Abnormal \
                     Closure, which §7.4.1 reserves for a network fault: {err:?}",
                ),
                WsRead::Silent => panic!(
                    "no Close frame within {:?} — the server neither closed nor spoke",
                    self.timeout,
                ),
            }
        }
    }

    /// Send a Close frame and read what comes back — RFC 6455 §5.5.1 obliges
    /// the receiving endpoint to answer with one.
    pub async fn close(&mut self, code: CloseCode, reason: &str) -> Option<(CloseCode, String)> {
        self.stream
            .send(ClientMessage::Close(Some(CloseFrame {
                code: u16::from(code).into(),
                reason: reason.to_string().into(),
            })))
            .await
            .expect("the socket accepts a close frame");
        // §5.5.1 obliges the peer to answer "as soon as practical", which does
        // not oblige it to answer *first* — a frame already in flight arrives
        // ahead of the Close. Reading exactly one frame here reported that as
        // "no Close frame", the same `None` as a peer that really sent none, so
        // this loops exactly as `expect_close` does.
        loop {
            match self.read_within(self.timeout).await {
                WsRead::Frame(WsFrame::Close(close)) => return close,
                WsRead::Frame(_) => {}
                WsRead::Silent | WsRead::Aborted(_) => return None,
            }
        }
    }
}

/// tungstenite's close frame in the vocabulary poem — and therefore the
/// gateway — states its codes in.
fn close(frame: CloseFrame) -> (CloseCode, String) {
    (u16::from(frame.code).into(), frame.reason.to_string())
}
