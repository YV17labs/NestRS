//! `nest-rs-reqwest` in process: real requests from a reqwest client through
//! the middleware to a TLS peer double whose certificate a `TestAuthority`
//! issued, which keeps the head of every request it reads.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]

mod middleware;

use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex};

use nest_rs_reqwest::ReqwestMiddleware;
use nest_rs_testing::{TestAuthority, TestCertificate};
use reqwest_middleware::ClientWithMiddleware;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// The authority the peer's certificate chains to, and the client trusts.
static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

static CERTIFICATE: LazyLock<TestCertificate> = LazyLock::new(|| AUTHORITY.server(&["127.0.0.1"]));

/// How the peer meets every request.
#[derive(Clone, Copy)]
enum Behaviour {
    /// Answer with this status and an empty body.
    Answer(u16),
    /// Read the request and never answer.
    Silent,
}

/// One request's header lines, names lowercased.
type Head = Vec<(String, String)>;

/// An HTTPS peer on loopback, one request per connection, keeping each
/// request's head.
struct Peer {
    addr: SocketAddr,
    heads: Arc<Mutex<Vec<Head>>>,
}

impl Peer {
    async fn serving(behaviour: Behaviour) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let addr = listener.local_addr().expect("its address");
        let heads = Arc::new(Mutex::new(Vec::new()));
        let acceptor = CERTIFICATE.acceptor(None);
        let kept = Arc::clone(&heads);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (acceptor, kept) = (acceptor.clone(), Arc::clone(&kept));
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(stream).await else {
                        return;
                    };
                    let mut head = Vec::new();
                    let mut buf = [0_u8; 1024];
                    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                        match tls.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => head.extend_from_slice(&buf[..read]),
                        }
                    }
                    kept.lock().expect("the heads").push(headers(&head));
                    match behaviour {
                        Behaviour::Answer(status) => {
                            let answer = format!(
                                "HTTP/1.1 {status} Answer\r\ncontent-length: 0\r\n\
                                 connection: close\r\n\r\n"
                            );
                            let _ = tls.write_all(answer.as_bytes()).await;
                            let _ = tls.shutdown().await;
                        }
                        Behaviour::Silent => std::future::pending::<()>().await,
                    }
                });
            }
        });
        Self { addr, heads }
    }

    /// `path` on the peer.
    fn url(&self, path: &str) -> String {
        format!("https://127.0.0.1:{}{path}", self.addr.port())
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }

    /// The value of the header `name` (lowercase) on the request the peer read
    /// `nth`.
    fn header(&self, nth: usize, name: &str) -> Option<String> {
        self.heads.lock().expect("the heads")[nth]
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.clone())
    }
}

/// The header lines of a request's head, names lowercased.
fn headers(head: &[u8]) -> Head {
    String::from_utf8_lossy(head)
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_owned()))
        .collect()
}

/// A client trusting the peer's authority, through the middleware, giving up
/// on an answer past `timeout`.
fn client(timeout: std::time::Duration) -> ClientWithMiddleware {
    let authority =
        reqwest::Certificate::from_pem(AUTHORITY.pem().as_bytes()).expect("the authority");
    let client = reqwest::Client::builder()
        .tls_certs_only([authority])
        .timeout(timeout)
        .build()
        .expect("a client");
    reqwest_middleware::ClientBuilder::new(client)
        .with(ReqwestMiddleware::new())
        .build()
}
