//! Test doubles: a real upgrade request, served by hyper over an in-memory pipe,
//! so its `OnUpgrade` yields a socket once answered.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Empty;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::oneshot;

type Answer = http::Response<Empty<Bytes>>;

/// An upgrade request as hyper hands it to a service, and the client across
/// the pipe.
pub(crate) struct Upgrading {
    /// The request, hyper's `OnUpgrade` in its extensions.
    pub(crate) request: http::Request<Incoming>,
    /// The client that sent it.
    pub(crate) peer: Peer,
}

/// The client end of an upgrade, waiting for the server's answer.
pub(crate) struct Peer {
    answer: Option<oneshot::Sender<Answer>>,
    client: DuplexStream,
}

impl Upgrading {
    /// Sends `GET /chat` asking to upgrade to `echo`, and waits for hyper to
    /// hand it over.
    pub(crate) async fn open() -> Self {
        let (mut client, server) = tokio::io::duplex(4096);
        let (request_tx, request_rx) = oneshot::channel();
        let (answer_tx, answer_rx) = oneshot::channel::<Answer>();
        let pending = Arc::new(Mutex::new(Some((request_tx, answer_rx))));
        let service = service_fn(move |request: http::Request<Incoming>| {
            let pending = pending.lock().unwrap().take();
            async move {
                let (request_tx, answer_rx) = pending.expect("one request per pipe");
                request_tx.send(request).expect("the test is waiting");
                Ok::<_, Infallible>(answer_rx.await.expect("the test answers"))
            }
        });
        tokio::spawn(
            http1::Builder::new()
                .serve_connection(TokioIo::new(server), service)
                .with_upgrades(),
        );
        client
            .write_all(
                b"GET /chat HTTP/1.1\r\nhost: api.example\r\nconnection: upgrade\r\nupgrade: echo\r\n\r\n",
            )
            .await
            .unwrap();
        let request = tokio::time::timeout(Duration::from_secs(5), request_rx)
            .await
            .expect("hyper hands the request over")
            .unwrap();
        Self {
            request,
            peer: Peer {
                answer: Some(answer_tx),
                client,
            },
        }
    }
}

impl Peer {
    /// Answers `101 Switching Protocols` and reads the answer's head on the
    /// client side, after which the pipe carries the upgraded protocol.
    pub(crate) async fn switch(&mut self) {
        let answer = http::Response::builder()
            .status(http::StatusCode::SWITCHING_PROTOCOLS)
            .header(http::header::CONNECTION, "upgrade")
            .header(http::header::UPGRADE, "echo")
            .body(Empty::new())
            .unwrap();
        self.answer.take().unwrap().send(answer).unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let byte = tokio::time::timeout(Duration::from_secs(5), self.client.read_u8())
                .await
                .expect("the server answers")
                .unwrap();
            head.push(byte);
        }
        assert!(head.starts_with(b"HTTP/1.1 101"), "{head:?}");
    }

    /// Writes `bytes` from the client into the upgraded protocol.
    pub(crate) async fn send(&mut self, bytes: &[u8]) {
        self.client.write_all(bytes).await.unwrap();
    }
}

/// Reads exactly `len` bytes from an upgraded socket.
pub(crate) async fn read_exactly(socket: &mut (impl AsyncRead + Unpin), len: usize) -> Vec<u8> {
    let mut read = vec![0; len];
    tokio::time::timeout(Duration::from_secs(5), socket.read_exact(&mut read))
        .await
        .expect("the bytes arrive")
        .unwrap();
    read
}
