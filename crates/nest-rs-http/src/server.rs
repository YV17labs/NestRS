//! The transport's accept loop: a place under the connection cap taken before
//! each `accept()`, accept failures backed off, TLS on tokio-rustls, and each
//! connection served by hyper on a task of its own, a panic contained there.
//!
//! poem stays the request engine behind [`BoxEndpoint`]; its `Server` exposes no
//! hook for the cap, the accept errors or hyper's timer, so the loop is ours
//! (`.claude/decisions/http-server.md`).

use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::panic::AssertUnwindSafe;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::FutureExt;
use http_body_util::combinators::BoxBody;
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use poem::endpoint::BoxEndpoint;
use poem::http::uri::Scheme;
use poem::web::{LocalAddr, RemoteAddr};
use poem::{Addr, Endpoint, Response};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

use crate::drain::Drain;
use crate::tls::ListenerTls;

/// What one HTTP/2 connection runs at once (RFC 9113 §5.1.2), hyper's own default.
const MAX_CONCURRENT_STREAMS: u32 = 200;

/// Reset streams a client may leave unanswered before the connection is
/// refused: the bound on rapid reset (CVE-2023-44487).
const MAX_PENDING_ACCEPT_RESET_STREAMS: usize = 20;

/// The decoded size of an HTTP/2 header list.
const MAX_HEADER_LIST_SIZE: u32 = 16 * 1024;

/// An HTTP/1 connection's read buffer, the sibling of the header-list cap: a
/// head that outgrows it is answered `431` (RFC 6585 §5) by hyper, where its
/// default buffer would let each slow head pin about 400 KiB.
const MAX_HTTP1_BUFFER: usize = 64 * 1024;

/// How often an idle HTTP/2 connection is pinged, and how long the ping waits.
const HTTP2_KEEP_ALIVE: Duration = Duration::from_secs(20);

/// What a response body is once it leaves poem.
type ResponseBody = BoxBody<Bytes, io::Error>;

/// The accept loop over one bound listener.
pub(crate) struct Server {
    listener: TcpListener,
    local: SocketAddr,
    tls: Option<ListenerTls>,
    endpoint: Arc<BoxEndpoint<'static, Response>>,
    drain: Arc<Drain>,
    admission: Admission,
}

impl Server {
    /// Serve `endpoint` on `listener`, at most `max_concurrent_connections` at once.
    pub(crate) fn new(
        listener: TcpListener,
        endpoint: BoxEndpoint<'static, Response>,
        drain: Arc<Drain>,
        max_concurrent_connections: usize,
        tls: Option<ListenerTls>,
    ) -> io::Result<Self> {
        Ok(Self {
            local: listener.local_addr()?,
            listener,
            tls,
            endpoint: Arc::new(endpoint),
            drain,
            admission: Admission::new(max_concurrent_connections),
        })
    }

    /// Accept until `signal` resolves; then stop accepting, ask every
    /// connection to finish what it is answering, wait for them until the
    /// drain's bound, and cut what is left.
    pub(crate) async fn run(mut self, signal: impl Future<Output = ()>) {
        let mut signal = pin!(signal);
        let (acceptor, _renewal) = match self.tls.take() {
            Some(tls) => (
                Some(tls.acceptor),
                tls.renewal
                    .map(|renewal| AbortOnDropHandle::new(tokio::spawn(renewal))),
            ),
            None => (None, None),
        };
        let builder = Arc::new(builder());
        let shutdown = CancellationToken::new();
        let mut connections = JoinSet::new();
        let mut backoff = AcceptBackoff::default();
        loop {
            let place = tokio::select! {
                () = &mut signal => break,
                place = self.admission.place() => match place {
                    Some(place) => place,
                    None => break,
                },
            };
            let (stream, peer) = tokio::select! {
                () = &mut signal => break,
                accepted = backoff.accept(|| self.listener.accept()) => accepted,
            };
            if let Err(error) = stream.set_nodelay(true) {
                tracing::warn!(
                    target: crate::target::HTTP,
                    error = %nest_rs_core::error_message(&error),
                    "TCP_NODELAY could not be set; the connection is served with Nagle's algorithm on",
                );
            }
            let connection = Connection {
                peer,
                local: self.local,
                endpoint: Arc::clone(&self.endpoint),
                builder: Arc::clone(&builder),
                shutdown: shutdown.clone(),
            };
            let socket = self.drain.socket(stream, place);
            connections.spawn(contained(connection.serve(socket, acceptor.clone())));
            while connections.try_join_next().is_some() {}
        }
        drop(self.listener);
        shutdown.cancel();
        if let Some(bound) = self.drain.bound()
            && tokio::time::timeout_at(bound, join_all(&mut connections))
                .await
                .is_err()
        {
            // Each socket drops past the bound, so the drain counts it as cut.
            connections.abort_all();
        }
        join_all(&mut connections).await;
    }
}

async fn join_all(connections: &mut JoinSet<()>) {
    while connections.join_next().await.is_some() {}
}

/// The settings every connection is served with: hyper's timer, the HTTP/2
/// limits, the HTTP/1 buffer. No head deadline is armed here.
fn builder() -> auto::Builder<Contained> {
    let mut builder = auto::Builder::new(Contained);
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(None)
        .keep_alive(true)
        .max_buf_size(MAX_HTTP1_BUFFER);
    builder
        .http2()
        .timer(TokioTimer::new())
        .max_concurrent_streams(MAX_CONCURRENT_STREAMS)
        .max_pending_accept_reset_streams(MAX_PENDING_ACCEPT_RESET_STREAMS)
        .max_header_list_size(MAX_HEADER_LIST_SIZE)
        .keep_alive_interval(HTTP2_KEEP_ALIVE)
        .keep_alive_timeout(HTTP2_KEEP_ALIVE);
    builder
}

/// One accepted socket's way through TLS and HTTP.
struct Connection {
    peer: SocketAddr,
    local: SocketAddr,
    endpoint: Arc<BoxEndpoint<'static, Response>>,
    builder: Arc<auto::Builder<Contained>>,
    shutdown: CancellationToken,
}

impl Connection {
    async fn serve<Io>(self, socket: Io, tls: Option<TlsAcceptor>)
    where
        Io: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let Some(acceptor) = tls else {
            return self.http(socket, Scheme::HTTP).await;
        };
        let handshake = tokio::select! {
            () = self.shutdown.cancelled() => return,
            handshake = acceptor.accept(socket) => handshake,
        };
        match handshake {
            Ok(stream) => self.http(stream, Scheme::HTTPS).await,
            Err(error) => tracing::debug!(
                target: crate::target::HTTP,
                error = %nest_rs_core::error_message(&error),
                "tls handshake failed; the connection is dropped",
            ),
        }
    }

    /// HTTP/1 or HTTP/2, told apart by the first bytes, until the peer goes or
    /// shutdown asks the connection to finish what it is answering.
    async fn http<Io>(self, io: Io, scheme: Scheme)
    where
        Io: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let Self {
            peer,
            local,
            endpoint,
            builder,
            shutdown,
        } = self;
        let service = hyper::service::service_fn(move |request: poem::http::Request<Incoming>| {
            let endpoint = Arc::clone(&endpoint);
            let request = poem::Request::from((
                request,
                LocalAddr(Addr::SocketAddr(local)),
                RemoteAddr(Addr::SocketAddr(peer)),
                scheme.clone(),
            ));
            async move {
                Ok::<_, Infallible>(poem::http::Response::<ResponseBody>::from(
                    endpoint.get_response(request).await,
                ))
            }
        });
        let mut connection =
            pin!(builder.serve_connection_with_upgrades(TokioIo::new(io), service));
        let ended = tokio::select! {
            ended = connection.as_mut() => ended,
            () = shutdown.cancelled() => {
                connection.as_mut().graceful_shutdown();
                connection.await
            }
        };
        if let Err(error) = ended {
            tracing::debug!(
                target: crate::target::HTTP,
                error = %nest_rs_core::error_message(&*error),
                "connection ended on an error",
            );
        }
    }
}

/// A connection's `future`, with a panic filed and kept from the task that
/// polls it: the connection is dropped, and its failure never reaches the loop
/// or the runtime.
async fn contained(future: impl Future<Output = ()>) {
    if let Err(payload) = AssertUnwindSafe(future).catch_unwind().await {
        nest_rs_core::contained_panic!(
            target: crate::target::HTTP,
            payload.as_ref(),
            "a connection task panicked",
        );
    }
}

/// hyper's executor for the tasks it spawns — an HTTP/2 connection's streams —
/// each contained on its own: a panic resets its stream, and the connection
/// keeps serving the others.
#[derive(Clone, Copy)]
struct Contained;

impl<F> hyper::rt::Executor<F> for Contained
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn execute(&self, future: F) {
        tokio::spawn(async move {
            if let Err(payload) = AssertUnwindSafe(future).catch_unwind().await {
                nest_rs_core::contained_panic!(
                    target: crate::target::HTTP,
                    payload.as_ref(),
                    "an HTTP/2 stream task panicked",
                );
            }
        });
    }
}

/// Each connection's place under the cap, taken before `accept()`, so a
/// connection past it waits in the kernel's listen backlog (Go's
/// `netutil.LimitListener`), never accepted to be reset.
struct Admission {
    places: Arc<Semaphore>,
    max_concurrent_connections: usize,
    /// The cap is reached and has not had a place free since.
    reached: bool,
}

impl Admission {
    fn new(max_concurrent_connections: usize) -> Self {
        Self {
            places: Arc::new(Semaphore::new(max_concurrent_connections)),
            max_concurrent_connections,
            reached: false,
        }
    }

    /// The next free place, waiting for one past the cap; one `warn` when the
    /// cap is reached, one `info` once a place is free again. `None` only if the
    /// places were closed, which nothing does.
    async fn place(&mut self) -> Option<OwnedSemaphorePermit> {
        if let Ok(place) = Arc::clone(&self.places).try_acquire_owned() {
            self.freed();
            return Some(place);
        }
        if !self.reached {
            self.reached = true;
            tracing::warn!(
                target: crate::target::HTTP,
                max_concurrent_connections = self.max_concurrent_connections,
                variable = crate::HttpConfig::connection_cap_variable(),
                "connection cap reached; new connections wait in the listen backlog",
            );
        }
        let place = Arc::clone(&self.places).acquire_owned().await.ok()?;
        if self.places.available_permits() > 0 {
            self.freed();
        }
        Some(place)
    }

    fn freed(&mut self) {
        if std::mem::take(&mut self.reached) {
            tracing::info!(
                target: crate::target::HTTP,
                max_concurrent_connections = self.max_concurrent_connections,
                "connection cap no longer reached; accepting again",
            );
        }
    }
}

/// How the loop answers an `accept()` that failed: a connection the peer gave
/// up on is skipped at once; anything else — out of file descriptors, of
/// buffers — waits 5 ms, doubling to 1 s, until an accept succeeds (Go's
/// `net/http`), so a full descriptor table is not spun on.
#[derive(Default)]
struct AcceptBackoff {
    /// The current delay; `None` outside an episode of failures.
    delay: Option<Duration>,
}

impl AcceptBackoff {
    const FIRST: Duration = Duration::from_millis(5);
    const MOST: Duration = Duration::from_secs(1);

    /// The next connection `accept` yields, each failure before it waited out.
    /// Dropped at the signal mid-wait: the listener's accept and the sleep are
    /// both cancel-safe.
    async fn accept<T, A>(&mut self, mut accept: impl FnMut() -> A) -> T
    where
        A: Future<Output = io::Result<T>>,
    {
        loop {
            match accept().await {
                Ok(accepted) => {
                    self.succeeded();
                    return accepted;
                }
                Err(error) => {
                    if let Some(delay) = self.failed(&error) {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
    }

    /// How long to wait before accepting again, if at all; one `warn` opens an
    /// episode.
    fn failed(&mut self, error: &io::Error) -> Option<Duration> {
        if matches!(
            error.kind(),
            io::ErrorKind::ConnectionAborted
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::Interrupted
        ) {
            tracing::debug!(
                target: crate::target::HTTP,
                error = %nest_rs_core::error_message(error),
                "a connection was gone before it was accepted; accepting the next",
            );
            return None;
        }
        let delay = match self.delay {
            Some(delay) => (delay * 2).min(Self::MOST),
            None => {
                tracing::warn!(
                    target: crate::target::HTTP,
                    error = %nest_rs_core::error_message(error),
                    retry_ms = u64::try_from(Self::FIRST.as_millis()).unwrap_or(u64::MAX),
                    "accept failed; backing off",
                );
                Self::FIRST
            }
        };
        self.delay = Some(delay);
        Some(delay)
    }

    /// An accept succeeded: the episode is over.
    fn succeeded(&mut self) {
        self.delay = None;
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_testing::LogCapture;

    use super::*;

    /// `EMFILE` on Linux and macOS alike.
    const EMFILE: i32 = 24;

    #[test]
    fn running_out_of_descriptors_backs_off_doubling_to_a_second_with_one_warn() {
        let logs = LogCapture::install();
        let mut backoff = AcceptBackoff::default();
        let emfile = io::Error::from_raw_os_error(EMFILE);
        let delays: Vec<u128> = (0..10)
            .map(|_| backoff.failed(&emfile).expect("backs off").as_millis())
            .collect();
        assert_eq!(delays, [5, 10, 20, 40, 80, 160, 320, 640, 1000, 1000]);
        let warned = logs.expect_one(crate::target::HTTP, "accept failed; backing off");
        assert_eq!(warned.level, "warn");
        assert_eq!(warned.field("retry_ms").as_deref(), Some("5"));
        assert!(warned.field("error").is_some(), "{:?}", warned.fields);

        backoff.succeeded();
        assert_eq!(
            backoff.failed(&emfile),
            Some(AcceptBackoff::FIRST),
            "an accept that succeeded ends the episode",
        );
        assert_eq!(
            logs.find(crate::target::HTTP, "accept failed; backing off")
                .len(),
            2,
            "and the next episode opens with its own warn",
        );
    }

    /// An accept failing with each of `errors` in turn, then succeeding.
    fn failing<const N: usize>(
        errors: [io::Error; N],
    ) -> impl FnMut() -> std::future::Ready<io::Result<()>> {
        let mut errors = errors.into_iter();
        move || std::future::ready(errors.next().map_or(Ok(()), Err))
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_accept_is_waited_out_before_the_next_one() {
        let logs = LogCapture::install();
        let mut backoff = AcceptBackoff::default();
        let start = tokio::time::Instant::now();
        backoff
            .accept(failing([EMFILE; 3].map(io::Error::from_raw_os_error)))
            .await;
        assert_eq!(start.elapsed(), Duration::from_millis(5 + 10 + 20));
        logs.expect_one(crate::target::HTTP, "accept failed; backing off");
    }

    #[tokio::test(start_paused = true)]
    async fn a_connection_the_peer_gave_up_on_is_skipped_at_once() {
        let logs = LogCapture::install();
        let mut backoff = AcceptBackoff::default();
        let gone = || {
            [
                io::ErrorKind::ConnectionAborted,
                io::ErrorKind::ConnectionReset,
                io::ErrorKind::Interrupted,
            ]
            .map(io::Error::from)
        };
        let start = tokio::time::Instant::now();
        backoff.accept(failing(gone())).await;
        assert_eq!(start.elapsed(), Duration::ZERO, "retried with no wait");
        logs.expect_none(crate::target::HTTP, "accept failed; backing off");

        let [aborted, reset, interrupted] = gone();
        backoff
            .accept(failing([
                aborted,
                reset,
                interrupted,
                io::Error::from_raw_os_error(EMFILE),
            ]))
            .await;
        assert_eq!(
            start.elapsed(),
            AcceptBackoff::FIRST,
            "and none of them opened an episode",
        );
    }

    #[tokio::test]
    async fn the_cap_is_said_once_when_reached_and_once_when_a_place_frees() {
        let logs = LogCapture::install();
        let mut admission = Admission::new(1);
        let first = admission.place().await.expect("a free place");
        let waiting = tokio::spawn(async move {
            let second = admission.place().await.expect("a place once one frees");
            (admission, second)
        });
        tokio::task::yield_now().await;
        logs.expect_one(
            crate::target::HTTP,
            "connection cap reached; new connections wait in the listen backlog",
        );
        drop(first);
        let (mut admission, second) = waiting.await.expect("the wait ends with a place");
        logs.expect_none(
            crate::target::HTTP,
            "connection cap no longer reached; accepting again",
        );
        drop(second);
        drop(admission.place().await);
        logs.expect_one(
            crate::target::HTTP,
            "connection cap no longer reached; accepting again",
        );
        logs.expect_one(
            crate::target::HTTP,
            "connection cap reached; new connections wait in the listen backlog",
        );
    }
}
