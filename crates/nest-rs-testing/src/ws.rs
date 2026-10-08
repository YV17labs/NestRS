//! Driving a WebSocket gateway over a **real** upgrade.
//!
//! The upgrade, the [`WsConfig`] it resolves, the lifetime ceiling, the writer
//! task and the per-message request scope all live in the connection task poem
//! spawns from `on_upgrade`, so this driver binds a port:
//! [`WsApp`](crate::ws::WsApp) boots the app's own HTTP transport on a free
//! local address, and [`WsSocket`](crate::ws::WsSocket) speaks the gateway's
//! `{ event, data }` envelope over it.
//!
//! ```
//! # use nest_rs_core::module;
//! # use nest_rs_testing::TestApp;
//! # use nest_rs_ws::{WsModule, gateway, input, messages};
//! # use serde_json::json;
//! #
//! # #[input]
//! # struct ChatMessage {
//! #     text: String,
//! # }
//! #
//! # #[gateway(path = "/ws")]
//! # #[derive(Default)]
//! # struct ChatGateway;
//! #
//! # #[messages]
//! # impl ChatGateway {
//! #     #[subscribe_message("message")]
//! #     #[public]
//! #     async fn message(&self, msg: ChatMessage) -> ChatMessage {
//! #         msg
//! #     }
//! # }
//! #
//! # #[module(imports = [WsModule], providers = [ChatGateway])]
//! # struct ChatModule;
//! #
//! # #[nest_rs_core::main]
//! # async fn main() -> anyhow::Result<()> {
//! # let token = String::from("a-token");
//! let app = TestApp::builder().module::<ChatModule>().build_ws().await?;
//! let mut socket = app.socket("/ws").bearer(&token).connect().await;
//! socket.send("message", json!({ "text": "hi" })).await;
//! assert_eq!(socket.next_envelope().await["event"], "message");
//! app.shutdown().await?;
//! # Ok(())
//! # }
//! ```
//!
//! [`WsSocket::expect_close`](crate::ws::WsSocket::expect_close) returns the
//! RFC 6455 §7.4.1 code the server ended the socket with.
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

use nest_rs_ws::CloseCode;

/// How long [`WsSocket::next_frame`] waits before reporting silence.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long [`WsSocketBuilder::connect`] keeps retrying the handshake: the
/// transport binds inside its own task, so the first attempt can land before
/// the listener exists.
const CONNECT_BUDGET: Duration = Duration::from_secs(5);

/// Between handshake attempts.
const CONNECT_BACKOFF: Duration = Duration::from_millis(20);

/// Reserve a free local address by binding one, reading it back, and letting it
/// go: [`HttpTransport`] binds inside `serve` and cannot report its port.
fn reserve_addr() -> Result<SocketAddr> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .context("no free local port for the test transport")?;
    let addr = listener.local_addr()?;
    drop(listener);
    Ok(addr)
}

pub(crate) async fn serve(app: HeadlessApp, transport: HttpTransport) -> Result<WsApp> {
    let addr = reserve_addr()?;
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

/// A booted app serving its HTTP surface on a real local port.
pub struct WsApp {
    app: HeadlessApp,
    handle: Option<TransportHandle>,
    addr: SocketAddr,
}

impl WsApp {
    /// The DI [`Container`], for resolving providers directly in assertions.
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

    /// Stop the transport and await its exit, surfacing its error. Dropping the
    /// [`WsApp`] instead detaches the task.
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
    /// Set a header on the **upgrade request**, where a gateway's
    /// connection-level guards run.
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

    /// Open the socket, reporting a refused handshake as an error; a TCP-level
    /// refusal is retried while the transport finishes binding.
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

/// One frame read off the socket; `Ping` and `Pong` are answered by the
/// protocol layer and never appear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsFrame {
    /// A text frame — a gateway's replies and pushes are all of these.
    Text(String),
    /// A binary frame.
    Binary(Vec<u8>),
    /// The close handshake, with the §7.4.1 code and reason when the peer sent
    /// them; `None` is a Close frame with no status (1005), not a missing frame.
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
    /// Send one `{ event, data }` envelope, encoded by the gateway's own
    /// [`WsEnvelope`].
    pub async fn send(&mut self, event: &str, data: Value) {
        let frame = WsEnvelope::encode(event, &data).expect("a JSON value re-encodes");
        self.send_text(frame).await;
    }

    /// Send a raw text frame, such as a malformed envelope or one past the cap.
    pub async fn send_text(&mut self, text: impl Into<String>) {
        self.stream
            .send(ClientMessage::Text(text.into().into()))
            .await
            .expect("the socket accepts a text frame");
    }

    /// Send a binary frame.
    pub async fn send_binary(&mut self, bytes: Vec<u8>) {
        self.stream
            .send(ClientMessage::Binary(bytes.into()))
            .await
            .expect("the socket accepts a binary frame");
    }

    /// The next `{ event, data }` envelope. Panics on silence or on a socket
    /// the server closed first.
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

    /// [`next_frame`](Self::next_frame) with an explicit budget; tell silence
    /// from an aborted socket through [`read_within`](Self::read_within).
    pub async fn next_frame_within(&mut self, within: Duration) -> Option<WsFrame> {
        match self.read_within(within).await {
            WsRead::Frame(frame) => Some(frame),
            WsRead::Silent | WsRead::Aborted(_) => None,
        }
    }

    /// The next frame, telling silence from a socket that died without a Close
    /// frame (RFC 6455 §7.4.1's **1006**).
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
                Ok(_) => continue,
                // Also where tungstenite reports a peer that violated framing.
                Err(err) => return WsRead::Aborted(Some(err.to_string())),
            }
        }
    }

    /// Assert nothing reaches the client within `within`; an aborted socket
    /// fails too.
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
    /// reason it carried; panics when the socket ends without one.
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
        // A frame already in flight may arrive ahead of the peer's Close.
        loop {
            match self.read_within(self.timeout).await {
                WsRead::Frame(WsFrame::Close(close)) => return close,
                WsRead::Frame(_) => {}
                WsRead::Silent | WsRead::Aborted(_) => return None,
            }
        }
    }
}

fn close(frame: CloseFrame) -> (CloseCode, String) {
    (u16::from(frame.code).into(), frame.reason.to_string())
}
