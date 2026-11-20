//! [`HttpsPeer`] — the server an HTTP client under test calls: a loopback HTTPS
//! peer presenting a [`TestCertificate`], answering from a script and keeping
//! what it was asked.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::tls::TestCertificate;

/// An HTTPS peer on loopback, for a suite whose subject is an HTTP client — an
/// app's own reqwest client, a middleware around it: one request per
/// connection, each answered from its script in turn, the last answer again
/// once the script ran out, and the head of every request kept.
///
/// ```
/// use nest_rs_testing::{HttpsPeer, PeerAnswer, TestAuthority};
///
/// # #[nest_rs_core::main]
/// # async fn main() {
/// let authority = TestAuthority::new();
/// let peer = HttpsPeer::answering(
///     &authority.server(&["127.0.0.1"]),
///     [
///         PeerAnswer::status(503).header("retry-after", "1"),
///         PeerAnswer::status(200),
///     ],
/// )
/// .await;
///
/// // A client trusting `authority.pem()` calls `peer.url("/parcels/42")`, then
/// // the suite reads what it was asked.
/// assert!(peer.url("/parcels/42").ends_with("/parcels/42"));
/// assert!(peer.requests().is_empty());
/// # }
/// ```
#[derive(Debug)]
pub struct HttpsPeer {
    addr: SocketAddr,
    heads: Arc<Mutex<Vec<RequestHead>>>,
}

/// One answer an [`HttpsPeer`] gives a request.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PeerAnswer {
    /// A status and its headers, with an empty body.
    Status {
        /// The status code.
        status: u16,
        /// The headers beside it, as written.
        headers: Vec<(String, String)>,
    },
    /// No answer: the request is read and the connection held open, as a peer
    /// that stopped answering holds it.
    Silent,
}

impl PeerAnswer {
    /// An answer of `status`, with no header but its length.
    pub const fn status(status: u16) -> Self {
        Self::Status {
            status,
            headers: Vec::new(),
        }
    }

    /// The same answer, carrying `name: value` too; a silent one stays silent.
    #[must_use]
    pub fn header(self, name: impl Into<String>, value: impl Into<String>) -> Self {
        match self {
            Self::Status {
                status,
                mut headers,
            } => {
                headers.push((name.into(), value.into()));
                Self::Status { status, headers }
            }
            Self::Silent => Self::Silent,
        }
    }
}

/// A request's line and its header lines, names lowercased.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RequestHead {
    /// The request line — `GET /parcels/42 HTTP/1.1`.
    pub line: String,
    /// Every header line, its name lowercased and both sides trimmed.
    pub headers: Vec<(String, String)>,
}

impl RequestHead {
    /// The value of the header `name` (lowercase), the first when it repeats.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }

    fn read(head: &[u8]) -> Self {
        let text = String::from_utf8_lossy(head);
        let mut lines = text.lines();
        Self {
            line: lines.next().unwrap_or_default().to_owned(),
            headers: lines
                .filter_map(|line| line.split_once(':'))
                .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_owned()))
                .collect(),
        }
    }
}

impl HttpsPeer {
    /// A peer presenting `certificate`, answering each request from `script` in
    /// turn and its last answer again once it ran out — `200` for an empty one.
    pub async fn answering(
        certificate: &TestCertificate,
        script: impl IntoIterator<Item = PeerAnswer>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port");
        let addr = listener.local_addr().expect("its address");
        let heads = Arc::new(Mutex::new(Vec::new()));
        let mut script: VecDeque<PeerAnswer> = script.into_iter().collect();
        if script.is_empty() {
            script.push_back(PeerAnswer::status(200));
        }
        let script = Arc::new(Mutex::new(script));
        let acceptor = certificate.acceptor(None);
        let kept = Arc::clone(&heads);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (acceptor, kept, script) =
                    (acceptor.clone(), Arc::clone(&kept), Arc::clone(&script));
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(stream).await else {
                        return;
                    };
                    let mut read = Vec::new();
                    let mut buf = [0_u8; 1024];
                    while !read.windows(4).any(|window| window == b"\r\n\r\n") {
                        match tls.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(count) => read.extend_from_slice(&buf[..count]),
                        }
                    }
                    lock(&kept).push(RequestHead::read(&read));
                    let answer = {
                        let mut script = lock(&script);
                        if script.len() > 1 {
                            script.pop_front()
                        } else {
                            script.front().cloned()
                        }
                    };
                    let Some(PeerAnswer::Status { status, headers }) = answer else {
                        std::future::pending::<()>().await;
                        return;
                    };
                    let mut answer = format!("HTTP/1.1 {status} Answer\r\ncontent-length: 0\r\n");
                    for (name, value) in headers {
                        answer.push_str(&format!("{name}: {value}\r\n"));
                    }
                    answer.push_str("connection: close\r\n\r\n");
                    if tls.write_all(answer.as_bytes()).await.is_ok() {
                        tls.shutdown().await.ok();
                    }
                });
            }
        });
        Self { addr, heads }
    }

    /// `path` on the peer, as a client addresses it.
    pub fn url(&self, path: &str) -> String {
        format!("https://127.0.0.1:{}{path}", self.addr.port())
    }

    /// The loopback port the peer listens on.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// The head of every request the peer read so far, in the order it read
    /// them.
    pub fn requests(&self) -> Vec<RequestHead> {
        lock(&self.heads).clone()
    }
}

fn lock<T>(held: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    held.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_silent_answer_stays_silent_whatever_it_is_told_to_carry() {
        assert_eq!(
            PeerAnswer::Silent.header("retry-after", "1"),
            PeerAnswer::Silent
        );
        assert_eq!(
            PeerAnswer::status(429).header("retry-after", "1"),
            PeerAnswer::Status {
                status: 429,
                headers: vec![("retry-after".to_owned(), "1".to_owned())],
            }
        );
    }

    #[test]
    fn a_head_reads_its_line_and_its_headers_by_lowercased_name() {
        let head =
            RequestHead::read(b"POST /parcels HTTP/1.1\r\nIdempotency-Key: k\r\nX-A:  b \r\n\r\n");
        assert_eq!(head.line, "POST /parcels HTTP/1.1");
        assert_eq!(head.header("idempotency-key"), Some("k"));
        assert_eq!(head.header("x-a"), Some("b"));
        assert_eq!(head.header("absent"), None);
    }
}
