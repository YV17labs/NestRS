//! A connection's phases and the deadline each one carries: the first head,
//! counted from accept; an idle connection, from its last answer; and on
//! HTTP/1 a head started after an idle spell, from its first byte. A request in
//! flight has none: its own deadlines bound it.
//!
//! hyper's own head timer is off (`header_read_timeout(None)`): it arms one
//! deadline over an idle connection and a head alike, and never over the
//! version sniff `auto::Builder` runs first.

use std::io::IoSlice;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, Result as IoResult};
use tokio::time::Instant;

use crate::window::Window;

/// What an HTTP/2 client sends first (RFC 9113 §3.4).
const H2_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// An instant not reached: no idle spell yet, or no head since the last one.
const NONE: u64 = u64::MAX;

/// The phase whose deadline passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// A request head did not arrive whole in time.
    Head,
    /// No request came in time after the last answer.
    Idle,
}

impl Phase {
    /// The name a log line files.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Head => "head",
            Self::Idle => "idle",
        }
    }
}

/// One connection's phases, shared by its reads, which start a head, its
/// requests, counted in flight until their bodies end, and its watch, which
/// tells when a phase outlived its deadline.
///
/// Both instants are nanoseconds since accept. On HTTP/1 everything runs on the
/// connection's task; on HTTP/2 a stream's task ends its request, so the count
/// publishes the idle instant it is stored after.
pub(crate) struct Phases {
    accepted: Instant,
    header_read: Duration,
    idle: Duration,
    in_flight: AtomicUsize,
    /// When the last request in flight ended; [`NONE`] before the first.
    idle_since: AtomicU64,
    /// When the head being read started: accept for the first, a byte read
    /// while idle for a later one; [`NONE`] when none has since `idle_since`.
    head_since: AtomicU64,
}

/// What the watch does next.
#[derive(Debug, PartialEq, Eq)]
enum Next {
    /// The phase outlived its deadline.
    Expired(Phase),
    /// Nothing is due before this instant.
    Check(Instant),
}

impl Phases {
    /// A connection accepted at `accepted`, in its first head.
    pub(crate) fn new(accepted: Instant, header_read: Duration, idle: Duration) -> Arc<Self> {
        Arc::new(Self {
            accepted,
            header_read,
            idle,
            in_flight: AtomicUsize::new(0),
            idle_since: AtomicU64::new(NONE),
            head_since: AtomicU64::new(0),
        })
    }

    /// When the first head is due — the TLS handshake inside it.
    pub(crate) fn first_head_due(&self) -> Instant {
        self.accepted + self.header_read
    }

    /// A request hyper handed the service: in flight until the returned guard
    /// ends or drops.
    pub(crate) fn request(self: &Arc<Self>) -> InFlight {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        InFlight(Some(Arc::clone(self)))
    }

    /// Resolves once a phase outlives its deadline, rechecking only when one
    /// could be due: no wake and no timer per request.
    pub(crate) async fn expired(&self) -> Phase {
        let mut check = self.accepted + self.soonest();
        loop {
            tokio::time::sleep_until(check).await;
            match self.next(Instant::now()) {
                Next::Expired(phase) => return phase,
                Next::Check(at) => check = at,
            }
        }
    }

    /// The soonest a phase starting now is due: nothing tells the watch when
    /// one starts, so it never sleeps longer than this.
    fn soonest(&self) -> Duration {
        self.header_read.min(self.idle)
    }

    fn next(&self, now: Instant) -> Next {
        let unstarted = now + self.soonest();
        if self.in_flight.load(Ordering::Acquire) > 0 {
            return Next::Check(unstarted);
        }
        let idle = self.at(self.idle_since.load(Ordering::Relaxed), self.idle);
        let head = self.at(self.head_since.load(Ordering::Relaxed), self.header_read);
        let (due, phase) = match (head, idle) {
            (Some(head), Some(idle)) if idle <= head => (idle, Phase::Idle),
            (Some(head), _) => (head, Phase::Head),
            (None, Some(idle)) => (idle, Phase::Idle),
            // Unreachable: a head runs from accept until the first idle spell.
            (None, None) => return Next::Check(unstarted),
        };
        if now >= due {
            return Next::Expired(phase);
        }
        Next::Check(due.min(unstarted))
    }

    /// `since` nanoseconds after accept, plus `deadline`; `None` for [`NONE`].
    fn at(&self, since: u64, deadline: Duration) -> Option<Instant> {
        (since != NONE).then(|| self.accepted + Duration::from_nanos(since) + deadline)
    }

    /// Nanoseconds from accept to `now`, never [`NONE`].
    fn since_accept(&self, now: Instant) -> u64 {
        u64::try_from(now.duration_since(self.accepted).as_nanos())
            .unwrap_or(NONE - 1)
            .min(NONE - 1)
    }

    /// The last request in flight ended: the connection is idle from `now`.
    fn idle_from(&self, now: Instant) {
        self.idle_since
            .store(self.since_accept(now), Ordering::Relaxed);
        self.head_since.store(NONE, Ordering::Relaxed);
    }

    /// An HTTP/1 read brought bytes: while idle, the first starts a head.
    fn read_bytes(&self) {
        if self.in_flight.load(Ordering::Relaxed) == 0
            && self.head_since.load(Ordering::Relaxed) == NONE
        {
            self.head_since
                .store(self.since_accept(Instant::now()), Ordering::Relaxed);
        }
    }
}

/// A request counted in flight, until its response body ends or drops — or
/// its handler's future, should the peer go before it answers.
pub(crate) struct InFlight(Option<Arc<Phases>>);

impl InFlight {
    fn end(&mut self) {
        if let Some(phases) = self.0.take() {
            // Stored before the count drops, which publishes it.
            phases.idle_from(Instant::now());
            phases.in_flight.fetch_sub(1, Ordering::Release);
        }
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.end();
    }
}

/// A response body whose request stays in flight until its last frame.
///
/// `size_hint` and `is_end_stream` are forwarded: hyper frames the response
/// from them. On HTTP/2 the body hands its stream's [`Window`] a piece at a
/// time.
pub(crate) struct PhasedBody<B> {
    inner: B,
    in_flight: InFlight,
    window: Option<Window>,
}

impl<B> PhasedBody<B> {
    pub(crate) fn new(inner: B, in_flight: InFlight, window: Option<Window>) -> Self {
        Self {
            inner,
            in_flight,
            window,
        }
    }
}

impl<B: Body<Data = Bytes> + Unpin> Body for PhasedBody<B> {
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, B::Error>>> {
        let this = self.get_mut();
        let Some(window) = &mut this.window else {
            let polled = Pin::new(&mut this.inner).poll_frame(cx);
            if let Poll::Ready(None) = polled {
                this.in_flight.end();
            }
            return polled;
        };
        window.came_back();
        let data = match window.rest() {
            Some(rest) => rest,
            None => match Pin::new(&mut this.inner).poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(data) => data,
                    Err(frame) => return Poll::Ready(Some(Ok(frame))),
                },
                Poll::Ready(None) => {
                    this.in_flight.end();
                    return Poll::Ready(None);
                }
                polled => return polled,
            },
        };
        Poll::Ready(Some(Ok(Frame::data(window.hand(data)))))
    }

    fn is_end_stream(&self) -> bool {
        self.window.as_ref().is_none_or(|window| window.held() == 0) && self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        let hint = self.inner.size_hint();
        let held = self.window.as_ref().map_or(0, Window::held) as u64;
        if held == 0 {
            return hint;
        }
        let mut with_held = SizeHint::new();
        if let Some(upper) = hint.upper() {
            with_held.set_upper(upper + held);
        }
        with_held.set_lower(hint.lower() + held);
        with_held
    }
}

/// What the reads have shown of the protocol.
#[derive(Clone, Copy)]
enum Protocol {
    /// This many bytes of the HTTP/2 preface read, and nothing else.
    Sniffing(usize),
    Http1,
    Http2,
}

/// The connection's I/O as hyper reads it — above TLS, so the bytes are
/// HTTP's: the HTTP/2 preface ends the first head, and on HTTP/1 a byte read
/// while idle starts the next.
pub(crate) struct PhasedIo<Io> {
    inner: Io,
    phases: Arc<Phases>,
    protocol: Protocol,
}

impl<Io> PhasedIo<Io> {
    pub(crate) fn new(inner: Io, phases: Arc<Phases>) -> Self {
        Self {
            inner,
            phases,
            protocol: Protocol::Sniffing(0),
        }
    }

    fn observe(&mut self, bytes: &[u8]) {
        match self.protocol {
            Protocol::Http1 => self.phases.read_bytes(),
            Protocol::Http2 => {}
            Protocol::Sniffing(matched) => {
                let rest = &H2_PREFACE[matched..];
                let compared = bytes.len().min(rest.len());
                self.protocol = if bytes[..compared] != rest[..compared] {
                    // Read inside the first head, whose deadline runs from accept.
                    Protocol::Http1
                } else if matched + compared == H2_PREFACE.len() {
                    // A stream's head is HEADERS, bounded by the stream; the
                    // connection is idle until it has one.
                    self.phases.idle_from(Instant::now());
                    Protocol::Http2
                } else {
                    Protocol::Sniffing(matched + compared)
                };
            }
        }
    }
}

impl<Io: AsyncRead + Unpin> AsyncRead for PhasedIo<Io> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<IoResult<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let polled = Pin::new(&mut this.inner).poll_read(cx, buf);
        if polled.is_ready() && buf.filled().len() > before {
            this.observe(&buf.filled()[before..]);
        }
        polled
    }
}

impl<Io: AsyncWrite + Unpin> AsyncWrite for PhasedIo<Io> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<IoResult<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<IoResult<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use http_body_util::{BodyExt, Full};
    use tokio::io::AsyncReadExt;

    use super::*;

    const HEAD: Duration = Duration::from_secs(30);
    const IDLE: Duration = Duration::from_secs(75);

    /// A connection accepted now.
    fn accepted() -> (Arc<Phases>, Instant) {
        let now = Instant::now();
        (Phases::new(now, HEAD, IDLE), now)
    }

    /// One request, answered now.
    fn answered(phases: &Arc<Phases>) {
        drop(phases.request());
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_head_is_due_from_accept() {
        let (phases, at) = accepted();
        assert_eq!(phases.first_head_due(), at + HEAD);
        assert_eq!(phases.next(at), Next::Check(at + HEAD));
        assert_eq!(phases.next(at + HEAD), Next::Expired(Phase::Head));
    }

    /// The watch still looks by the time a phase starting now would be due.
    #[tokio::test(start_paused = true)]
    async fn a_request_in_flight_has_no_connection_deadline() {
        let (phases, at) = accepted();
        let request = phases.request();
        let late = at + 10 * IDLE;
        assert_eq!(phases.next(late), Next::Check(late + HEAD));
        drop(request);
    }

    /// While no head has started, the watch looks again by the time one
    /// started now would be due.
    #[tokio::test(start_paused = true)]
    async fn an_idle_connection_is_due_from_its_last_answer() {
        let (phases, at) = accepted();
        tokio::time::advance(Duration::from_secs(5)).await;
        answered(&phases);
        let idle = at + Duration::from_secs(5);
        assert_eq!(phases.next(idle), Next::Check(idle + HEAD));
        assert_eq!(phases.next(idle + HEAD), Next::Check(idle + 2 * HEAD));
        assert_eq!(phases.next(idle + 2 * HEAD), Next::Check(idle + IDLE));
        assert_eq!(phases.next(idle + IDLE), Next::Expired(Phase::Idle));
    }

    #[tokio::test(start_paused = true)]
    async fn an_http1_head_after_an_idle_spell_is_due_from_its_first_byte() {
        let (phases, at) = accepted();
        let mut io = PhasedIo::new(
            (&b"GET / HTTP/1.1\r\n\r\n"[..]).chain(&b"GET /next"[..]),
            Arc::clone(&phases),
        );
        let mut buf = [0_u8; 64];
        io.read_exact(&mut buf[..18]).await.expect("the first head");
        assert_eq!(
            phases.next(at + HEAD),
            Next::Expired(Phase::Head),
            "read inside the first head, which still runs from accept",
        );
        answered(&phases);
        tokio::time::advance(Duration::from_secs(10)).await;
        io.read_exact(&mut buf[..9])
            .await
            .expect("the next head's first bytes");
        let head = at + Duration::from_secs(10);
        assert_eq!(
            phases.next(head + HEAD - Duration::from_millis(1)),
            Next::Check(head + HEAD)
        );
        assert_eq!(phases.next(head + HEAD), Next::Expired(Phase::Head));
    }

    /// Whichever is due first: a head starting late in an idle spell is cut
    /// with it.
    #[tokio::test(start_paused = true)]
    async fn a_head_started_late_in_an_idle_spell_is_cut_at_the_idle_deadline() {
        let (phases, at) = accepted();
        answered(&phases);
        tokio::time::advance(IDLE - Duration::from_secs(10)).await;
        phases.read_bytes();
        assert_eq!(phases.next(at + IDLE), Next::Expired(Phase::Idle));
    }

    #[tokio::test(start_paused = true)]
    async fn the_http2_preface_ends_the_first_head_and_starts_an_idle_spell() {
        let (phases, at) = accepted();
        let (first, rest) = H2_PREFACE.split_at(10);
        let mut io = PhasedIo::new(first.chain(rest), Arc::clone(&phases));
        let mut buf = [0_u8; 64];
        tokio::time::advance(Duration::from_secs(1)).await;
        io.read_exact(&mut buf[..10])
            .await
            .expect("part of the preface");
        assert_eq!(
            phases.next(at + HEAD),
            Next::Expired(Phase::Head),
            "not whole yet"
        );
        io.read_exact(&mut buf[..14]).await.expect("the rest of it");
        let idle = at + Duration::from_secs(1);
        assert!(
            matches!(phases.next(at + HEAD), Next::Check(_)),
            "the first head ended with the preface",
        );
        assert_eq!(
            phases.next(idle + IDLE - Duration::from_millis(1)),
            Next::Check(idle + IDLE)
        );
        assert_eq!(phases.next(idle + IDLE), Next::Expired(Phase::Idle));
    }

    /// The watch run from accept; resolves with the phase it cut and when.
    fn watch(phases: &Arc<Phases>) -> tokio::task::JoinHandle<(Phase, Instant)> {
        let phases = Arc::clone(phases);
        tokio::spawn(async move { (phases.expired().await, Instant::now()) })
    }

    /// A head that starts after a request the watch saw in flight is cut at
    /// its own deadline, not an idle span after that look.
    #[tokio::test(start_paused = true)]
    async fn a_head_after_a_request_in_flight_at_a_look_is_cut_at_its_own_deadline() {
        let (phases, at) = accepted();
        let request = phases.request();
        let watch = watch(&phases);
        tokio::time::sleep(HEAD + Duration::from_secs(10)).await;
        drop(request);
        tokio::time::sleep(Duration::from_secs(1)).await;
        phases.read_bytes();
        let (phase, cut) = watch.await.expect("the watch");
        assert_eq!(
            (phase, cut - at),
            (Phase::Head, HEAD + Duration::from_secs(11) + HEAD),
            "cut at the head's deadline, counted from accept",
        );
    }

    /// An idle deadline shorter than the head one: a spell started inside the
    /// first head's deadline is cut at its own.
    #[tokio::test(start_paused = true)]
    async fn an_idle_deadline_shorter_than_the_head_one_is_cut_at_its_own() {
        let at = Instant::now();
        let idle = Duration::from_secs(5);
        let phases = Phases::new(at, HEAD, idle);
        let watch = watch(&phases);
        tokio::time::sleep(Duration::from_secs(1)).await;
        answered(&phases);
        let (phase, cut) = watch.await.expect("the watch");
        assert_eq!(
            (phase, cut - at),
            (Phase::Idle, Duration::from_secs(1) + idle),
            "cut at the idle spell's deadline, counted from accept",
        );
    }

    #[tokio::test]
    async fn a_request_ends_once_at_its_body_end_or_its_drop() {
        let (phases, _) = accepted();
        let mut body = PhasedBody::new(
            Full::new(Bytes::from_static(b"pong")),
            phases.request(),
            None,
        );
        assert_eq!(phases.in_flight.load(Ordering::Acquire), 1);
        body.frame().await.expect("one frame").expect("data");
        assert!(body.frame().await.is_none());
        assert_eq!(
            phases.in_flight.load(Ordering::Acquire),
            0,
            "ended at its last frame"
        );
        drop(body);
        assert_eq!(
            phases.in_flight.load(Ordering::Acquire),
            0,
            "and not again at its drop"
        );

        drop(PhasedBody::new(
            Full::new(Bytes::new()),
            phases.request(),
            None,
        ));
        assert_eq!(
            phases.in_flight.load(Ordering::Acquire),
            0,
            "a body never polled ends at its drop"
        );
    }
}
