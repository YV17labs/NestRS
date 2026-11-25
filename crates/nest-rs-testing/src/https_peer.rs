//! [`HttpsPeer`] — the server an HTTP client under test calls: a loopback HTTPS
//! peer presenting a [`TestCertificate`], answering from a script and keeping
//! what it was asked.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

use crate::tls::TestCertificate;

/// An HTTPS peer on loopback, for a suite whose subject is an HTTP client — an
/// app's own reqwest client, a middleware around it: one request per
/// connection, each answered from its script in turn, the last answer again
/// once the script ran out, and the head of every request kept. An answer's
/// body comes whole, cut short, stalled, or in two parts the suite lets
/// through one at a time ([`release`](Self::release)), and its head carries
/// the headers the suite gives it ([`PeerAnswer::header`]), whichever it is.
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
    /// Closed once the held-back parts are released: a closed semaphore wakes
    /// every part waiting on it, and each one that comes after.
    held_back: Arc<Semaphore>,
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
    /// A status announcing a body the peer starts and never ends: the head,
    /// half the bytes its `content-length` declared, then the connection held
    /// open, as a peer that stopped answering mid-body holds it.
    Stalling {
        /// The status code.
        status: u16,
        /// The headers beside it, as written.
        headers: Vec<(String, String)>,
    },
    /// A status and a whole body, its length declared.
    Body {
        /// The status code.
        status: u16,
        /// The headers beside it, as written.
        headers: Vec<(String, String)>,
        /// The body, as written.
        body: Vec<u8>,
    },
    /// A status announcing a body the peer cuts short: the head, half the
    /// bytes its `content-length` declared, then the connection closed, as a
    /// peer that went down mid-body leaves it.
    Truncated {
        /// The status code.
        status: u16,
        /// The headers beside it, as written.
        headers: Vec<(String, String)>,
    },
    /// A status and a body written in two parts: the head and the first part,
    /// then the second once the suite calls [`HttpsPeer::release`], and the
    /// connection closed.
    Parted {
        /// The status code.
        status: u16,
        /// The headers beside it, as written.
        headers: Vec<(String, String)>,
        /// The part written with the head.
        first: Vec<u8>,
        /// The part held back until the peer is released.
        second: Vec<u8>,
    },
}

impl PeerAnswer {
    /// An answer of `status`, with no header but its length.
    pub const fn status(status: u16) -> Self {
        Self::Status {
            status,
            headers: Vec::new(),
        }
    }

    /// An answer of `status` whose body stalls halfway.
    pub const fn stalling(status: u16) -> Self {
        Self::Stalling {
            status,
            headers: Vec::new(),
        }
    }

    /// An answer of `status` carrying `body` whole.
    pub fn body(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self::Body {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// An answer of `status` whose body is cut short halfway, the connection
    /// closed.
    pub const fn truncated(status: u16) -> Self {
        Self::Truncated {
            status,
            headers: Vec::new(),
        }
    }

    /// An answer of `status` whose body is `first`, then `second` once the
    /// peer is [released](HttpsPeer::release).
    pub fn parted(status: u16, first: impl Into<Vec<u8>>, second: impl Into<Vec<u8>>) -> Self {
        Self::Parted {
            status,
            headers: Vec::new(),
            first: first.into(),
            second: second.into(),
        }
    }

    /// The same answer, carrying `name: value` too, written in its head
    /// beside the length it declares, whatever becomes of its body. A
    /// [`Silent`](Self::Silent) answer writes no head, so the header has
    /// nowhere to go: it stays as it is.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let headers = match &mut self {
            Self::Status { headers, .. }
            | Self::Stalling { headers, .. }
            | Self::Body { headers, .. }
            | Self::Truncated { headers, .. }
            | Self::Parted { headers, .. } => headers,
            Self::Silent => return self,
        };
        headers.push((name.into(), value.into()));
        self
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
        let held_back = Arc::new(Semaphore::new(0));
        let released = Arc::clone(&held_back);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (acceptor, kept, script, released) = (
                    acceptor.clone(),
                    Arc::clone(&kept),
                    Arc::clone(&script),
                    Arc::clone(&released),
                );
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
                    // What is written at once of a body of `declared` bytes,
                    // what follows once the peer is released, and whether the
                    // connection is closed after them or held open.
                    let (status, headers, declared, first, second, closed) = match answer {
                        Some(PeerAnswer::Status { status, headers }) => {
                            (status, headers, 0, Vec::new(), None, true)
                        }
                        Some(PeerAnswer::Body {
                            status,
                            headers,
                            body,
                        }) => (status, headers, body.len(), body, None, true),
                        Some(PeerAnswer::Stalling { status, headers }) => {
                            let declared = 2 * HALF.len();
                            (status, headers, declared, HALF.to_vec(), None, false)
                        }
                        Some(PeerAnswer::Truncated { status, headers }) => {
                            let declared = 2 * HALF.len();
                            (status, headers, declared, HALF.to_vec(), None, true)
                        }
                        Some(PeerAnswer::Parted {
                            status,
                            headers,
                            first,
                            second,
                        }) => {
                            let declared = first.len() + second.len();
                            (status, headers, declared, first, Some(second), true)
                        }
                        Some(PeerAnswer::Silent) | None => {
                            std::future::pending::<()>().await;
                            return;
                        }
                    };
                    let mut head =
                        format!("HTTP/1.1 {status} Answer\r\ncontent-length: {declared}\r\n");
                    for (name, value) in headers {
                        head.push_str(&format!("{name}: {value}\r\n"));
                    }
                    head.push_str("connection: close\r\n\r\n");
                    let mut written = tls.write_all(head.as_bytes()).await.is_ok()
                        && tls.write_all(&first).await.is_ok()
                        && tls.flush().await.is_ok();
                    if let Some(second) = second {
                        // Closed, never handed a permit: its one answer is the release.
                        released.acquire().await.map(drop).ok();
                        written = written
                            && tls.write_all(&second).await.is_ok()
                            && tls.flush().await.is_ok();
                    }
                    if !closed {
                        std::future::pending::<()>().await;
                    }
                    if written {
                        tls.shutdown().await.ok();
                    }
                });
            }
        });
        Self {
            addr,
            heads,
            held_back,
        }
    }

    /// Let every part an answer holds back be written: the second of each
    /// [`PeerAnswer::parted`] body, those already waiting and those to come.
    pub fn release(&self) {
        self.held_back.close();
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

/// What a [`PeerAnswer::Stalling`] and a [`PeerAnswer::Truncated`] write of
/// their body before it stalls or ends: half of what its head declared.
const HALF: &[u8] = b"half of an answer";

fn lock<T>(held: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    held.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One answer of each kind that writes a head.
    fn answers_with_a_head() -> [PeerAnswer; 5] {
        [
            PeerAnswer::status(503),
            PeerAnswer::stalling(503),
            PeerAnswer::body(503, "a parcel"),
            PeerAnswer::truncated(503),
            PeerAnswer::parted(503, "a par", "cel"),
        ]
    }

    /// A silent answer writes no head, so a header has nowhere to go.
    #[test]
    fn a_silent_answer_stays_silent_whatever_it_is_told_to_carry() {
        assert_eq!(
            PeerAnswer::Silent.header("retry-after", "1"),
            PeerAnswer::Silent
        );
    }

    #[test]
    fn every_answer_that_writes_a_head_keeps_the_headers_it_is_told_to_carry() {
        assert_eq!(
            PeerAnswer::status(429).header("retry-after", "1"),
            PeerAnswer::Status {
                status: 429,
                headers: vec![("retry-after".to_owned(), "1".to_owned())],
            }
        );
        for answer in answers_with_a_head() {
            let carrying = answer.clone().header("retry-after", "2");
            assert_ne!(carrying, answer);
            assert_ne!(carrying.clone().header("x-attempt", "1"), carrying);
        }
    }

    /// The head `answer` is given by a peer scripted with it, read off the
    /// socket as it is written, up to the blank line ending it.
    async fn head_of(answer: PeerAnswer) -> String {
        use tokio_rustls::rustls::pki_types::pem::PemObject as _;
        use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
        use tokio_rustls::rustls::{ClientConfig, RootCertStore};

        let authority = crate::TestAuthority::new();
        let peer = HttpsPeer::answering(&authority.server(&["127.0.0.1"]), [answer]).await;
        let mut roots = RootCertStore::empty();
        roots.add_parsable_certificates(
            CertificateDer::pem_slice_iter(authority.pem().as_bytes()).map_while(Result::ok),
        );
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", peer.port()))
            .await
            .expect("the peer listens");
        let mut tls = tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(
                ServerName::try_from("127.0.0.1").expect("an address"),
                stream,
            )
            .await
            .expect("the peer's certificate is the authority's");
        tls.write_all(b"GET /parcels/42 HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n")
            .await
            .expect("the request is written");
        let mut read = Vec::new();
        let mut buf = [0_u8; 1024];
        let ends = loop {
            if let Some(ends) = read.windows(4).position(|window| window == b"\r\n\r\n") {
                break ends;
            }
            let count = tls.read(&mut buf).await.expect("the head is read");
            assert_ne!(count, 0, "the connection closed before the head ended");
            read.extend_from_slice(&buf[..count]);
        };
        String::from_utf8(read[..ends].to_vec()).expect("a head is text")
    }

    /// Each answer writes the headers it carries in its head, beside the
    /// length it declares, whatever becomes of its body.
    #[tokio::test]
    async fn every_answer_that_writes_a_head_writes_the_headers_it_carries() {
        for (answer, declared) in answers_with_a_head().into_iter().zip([
            0,
            2 * HALF.len(),
            "a parcel".len(),
            2 * HALF.len(),
            "a parcel".len(),
        ]) {
            let carrying = answer
                .clone()
                .header("retry-after", "2")
                .header("x-attempt", "1");
            let head = head_of(carrying).await;
            let lines: Vec<&str> = head.split("\r\n").collect();
            assert_eq!(
                lines,
                [
                    "HTTP/1.1 503 Answer",
                    &format!("content-length: {declared}"),
                    "retry-after: 2",
                    "x-attempt: 1",
                    "connection: close",
                ],
                "{answer:?}"
            );
        }
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
