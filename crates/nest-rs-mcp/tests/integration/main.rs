//! Integration tests mirroring `src/` — one binary, one module per concern.
//!
//! The fixtures below are the ones the shutdown tests share: a transport served
//! on a real loopback port, and raw HTTP/1.1 the way a client that holds a
//! stream open speaks it. `TestClient` has no connection for a shutdown window
//! to close, so these tests cannot use it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod endpoint;
mod error;
mod guard;
mod mcp_impl;
mod operation;
mod parameters;
mod propagate;
mod registry;
mod scope;

use std::net::TcpListener as StdTcpListener;
use std::time::Duration;

use nest_rs_core::{App, Module, Transport};
use nest_rs_http::{HttpConfig, HttpTransport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// `M` served on a free loopback port by the transport `HttpModule` would build,
/// with `window` as its shutdown window.
pub(crate) struct Serving {
    pub(crate) port: u16,
    cancel: CancellationToken,
    task: JoinHandle<nest_rs_core::anyhow::Result<()>>,
}

pub(crate) async fn serve_on_loopback<M: Module + 'static>(window: Duration) -> Serving {
    let app = App::builder()
        .module::<M>()
        .build()
        .await
        .expect("the module boots");
    let port = StdTcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .expect("an ephemeral port")
        .port();
    let mut transport = HttpTransport::from_config(&HttpConfig {
        host: "127.0.0.1".into(),
        port,
        shutdown_timeout: window,
        ..HttpConfig::default()
    })
    .expect("the config builds a transport");
    transport
        .configure(app.container())
        .await
        .expect("the transport configures");
    let cancel = CancellationToken::new();
    let task = tokio::spawn({
        let cancel = cancel.clone();
        async move { Box::new(transport).serve(cancel).await }
    });
    Serving { port, cancel, task }
}

impl Serving {
    /// Ask for shutdown with the clock paused — nothing reads a socket
    /// meanwhile — and return how long `serve` took to come back on that clock.
    pub(crate) async fn stop(self) -> Duration {
        tokio::time::pause();
        let asked = tokio::time::Instant::now();
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

/// A fresh connection to the served port, retried while the listener comes up.
async fn connect(port: u16) -> TcpStream {
    for _ in 0..100 {
        if let Ok(stream) = TcpStream::connect(("127.0.0.1", port)).await {
            return stream;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the transport never came up on port {port}");
}

/// Write `request` on a fresh connection and read the response head, leaving
/// the connection — and any stream it carries — open.
pub(crate) async fn exchange(port: u16, request: &str) -> (TcpStream, String) {
    let mut stream = connect(port).await;
    stream
        .write_all(request.as_bytes())
        .await
        .expect("the request is sent");
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut byte).await.expect("the head is readable");
        assert!(read > 0, "the connection ended inside the head");
        head.push(byte[0]);
    }
    (stream, String::from_utf8_lossy(&head).into_owned())
}

fn session_header(session: Option<&str>) -> String {
    session
        .map(|id| format!("mcp-session-id: {id}\r\n"))
        .unwrap_or_default()
}

/// POST one JSON-RPC message to `path` over raw HTTP/1.1.
pub(crate) async fn post(
    port: u16,
    path: &str,
    session: Option<&str>,
    body: &serde_json::Value,
) -> (TcpStream, String) {
    let body = body.to_string();
    let session = session_header(session);
    exchange(
        port,
        &format!(
            "POST {path} HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n\
             accept: application/json, text/event-stream\r\n{session}content-length: {}\r\n\r\n{body}",
            body.len(),
        ),
    )
    .await
}

/// Open a stateful session on `path` — `initialize`, then
/// `notifications/initialized` — and return its id.
pub(crate) async fn open_raw_session(port: u16, path: &str) -> String {
    let (_, head) = post(
        port,
        path,
        None,
        &nest_rs_testing::mcp::initialize_request(),
    )
    .await;
    let session = head
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("mcp-session-id: ")
                .map(|_| line["mcp-session-id: ".len()..].trim().to_owned())
        })
        .unwrap_or_else(|| panic!("initialize opens a session: {head}"));
    post(
        port,
        path,
        Some(&session),
        &serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    session
}

/// Open the session's standalone `GET` stream — what a client holds to hear
/// what the server pushes.
pub(crate) async fn get_stream(port: u16, path: &str, session: &str) -> (TcpStream, String) {
    let session = session_header(Some(session));
    exchange(
        port,
        &format!(
            "GET {path} HTTP/1.1\r\nhost: localhost\r\naccept: text/event-stream\r\n{session}\r\n"
        ),
    )
    .await
}

/// Everything left on the socket, up to its end.
pub(crate) async fn read_to_end(stream: &mut TcpStream) -> String {
    let mut rest = Vec::new();
    match tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut rest)).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        Ok(Err(error)) => panic!("the socket failed rather than ending: {error}"),
        Err(_) => panic!("the server never closed the connection"),
    }
    String::from_utf8_lossy(&rest).into_owned()
}
