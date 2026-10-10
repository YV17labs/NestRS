//! The transport's way down as the connections it serves see it: the instant
//! shutdown is asked for, the instant the window closes, and what is still open
//! at the second.
//!
//! Every accepted socket is counted until it drops, an upgraded one included:
//! it outlives the HTTP connection that upgraded it. Below TLS, its writes are
//! all the connection's — the handshake's, a body's, a WebSocket's frames — so
//! the send deadline is held here.

use std::future::Future;
use std::io::{Error as IoError, ErrorKind, IoSlice};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, Result as IoResult};
use tokio::sync::OwnedSemaphorePermit;
use tokio::time::{Instant, Sleep};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

/// The sockets a transport has accepted and not yet dropped, how many of them
/// the shutdown window closed, and the two instants of its way down.
#[derive(Default)]
pub(crate) struct Drain {
    open: AtomicUsize,
    closed_at_bound: AtomicUsize,
    /// When the window elapses; set once, when shutdown is asked for.
    bound: OnceLock<Instant>,
    /// Cancelled when shutdown is asked for.
    going_away: CancellationToken,
}

impl Drain {
    /// `io`, counted open until it drops, and holding `place` — its place under
    /// the connection cap — as long.
    pub(crate) fn socket<Io>(
        self: &Arc<Self>,
        io: Io,
        place: OwnedSemaphorePermit,
        send_timeout: Duration,
    ) -> DrainSocket<Io> {
        self.opened();
        DrainSocket {
            inner: io,
            drain: Arc::clone(self),
            _place: place,
            send_timeout,
            stall: None,
            stalled: false,
        }
    }

    /// Shutdown was asked for: the window starts now.
    ///
    /// Called before the accept loop stops, so every socket it cuts at the
    /// bound drops after this instant.
    pub(crate) fn begin(&self, window: Duration) {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a second shutdown signal keeps the first window"
        )]
        let _ = self.bound.set(Instant::now() + window);
        self.going_away.cancel();
    }

    /// Resolves once shutdown has been asked for.
    pub(crate) fn going_away(&self) -> WaitForCancellationFutureOwned {
        self.going_away.clone().cancelled_owned()
    }

    /// When the window closes — `None` until shutdown is asked for.
    pub(crate) fn bound(&self) -> Option<Instant> {
        self.bound.get().copied()
    }

    /// The window has closed: whatever is dropped from now on is cut by it.
    pub(crate) fn is_past_bound(&self) -> bool {
        self.bound().is_some_and(|bound| Instant::now() >= bound)
    }

    /// Say what the window left behind, once the server has stopped: how many
    /// connections it closed, and how many upgraded ones outlive the transport.
    pub(crate) fn report(&self, window: Duration) {
        let cut = self.closed_at_bound.load(Ordering::Acquire);
        let upgraded_open = self.open.load(Ordering::Acquire);
        if cut > 0 {
            tracing::warn!(
                target: crate::target::HTTP,
                cut,
                upgraded_open,
                shutdown_timeout_ms = u64::try_from(window.as_millis()).unwrap_or(u64::MAX),
                "connections still open as the shutdown window closes are cut; a request still \
                 running is dropped unanswered and a stream ends mid-flow",
            );
        } else if upgraded_open > 0 {
            tracing::info!(
                target: crate::target::HTTP,
                upgraded_open,
                "HTTP transport stopped with upgraded connections still open; they end with their \
                 handlers or with the process",
            );
        }
    }

    fn opened(&self) {
        self.open.fetch_add(1, Ordering::AcqRel);
    }

    fn closed(&self) {
        if self.is_past_bound() {
            self.closed_at_bound.fetch_add(1, Ordering::AcqRel);
        }
        self.open.fetch_sub(1, Ordering::AcqRel);
    }
}

/// One accepted socket, counted open until it is dropped, whose write fails
/// `TimedOut` once the peer has taken nothing for `send_timeout`. Vectored
/// writes are forwarded too: hyper uses them when the socket offers them.
pub(crate) struct DrainSocket<Io> {
    inner: Io,
    drain: Arc<Drain>,
    _place: OwnedSemaphorePermit,
    send_timeout: Duration,
    /// Allocated the first time a write waits, and kept for the next.
    stall: Option<Pin<Box<Sleep>>>,
    /// A write is waiting, and `stall` holds its deadline.
    stalled: bool,
}

impl<Io> DrainSocket<Io> {
    /// A write came back `polled`: one that went through clears the stall.
    fn written<T>(&mut self, cx: &mut Context<'_>, polled: Poll<IoResult<T>>) -> Poll<IoResult<T>> {
        if polled.is_ready() {
            self.stalled = false;
            return polled;
        }
        self.stall(cx)
    }

    /// A write or flush waits: the stall is armed — once, so it counts from
    /// the first wait — and the wait fails past it.
    fn stall<T>(&mut self, cx: &mut Context<'_>) -> Poll<IoResult<T>> {
        let stall = match &mut self.stall {
            Some(stall) if self.stalled => stall,
            Some(stall) => {
                stall.as_mut().reset(Instant::now() + self.send_timeout);
                stall
            }
            None => self
                .stall
                .insert(Box::pin(tokio::time::sleep(self.send_timeout))),
        };
        self.stalled = true;
        match stall.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(IoError::new(
                ErrorKind::TimedOut,
                "the peer took nothing within the send deadline",
            ))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<Io> Drop for DrainSocket<Io> {
    fn drop(&mut self) {
        self.drain.closed();
    }
}

impl<Io: AsyncRead + Unpin> AsyncRead for DrainSocket<Io> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<IoResult<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<Io: AsyncWrite + Unpin> AsyncWrite for DrainSocket<Io> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<IoResult<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write(cx, buf);
        self.written(cx, polled)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<IoResult<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        self.written(cx, polled)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        // A flush that completes is no progress of its own: TCP's always does.
        match Pin::new(&mut self.inner).poll_flush(cx) {
            Poll::Pending => self.stall(cx),
            flushed => flushed,
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<IoResult<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEND: Duration = Duration::from_secs(60);

    fn socket(drain: &Arc<Drain>) -> DrainSocket<tokio::io::DuplexStream> {
        let places = Arc::new(tokio::sync::Semaphore::new(1));
        let place = places.try_acquire_owned().expect("a free place");
        drain.socket(tokio::io::duplex(8).0, place, SEND)
    }

    #[tokio::test(start_paused = true)]
    async fn a_socket_counts_open_until_it_is_dropped_and_as_cut_only_past_the_bound() {
        let drain = Arc::new(Drain::default());
        let early = socket(&drain);
        let late = socket(&drain);
        let kept = socket(&drain);
        assert_eq!(drain.open.load(Ordering::Acquire), 3);

        drain.begin(Duration::from_secs(25));
        drop(early);
        tokio::time::advance(Duration::from_secs(25)).await;
        drop(late);

        assert_eq!(
            drain.closed_at_bound.load(Ordering::Acquire),
            1,
            "only the socket dropped once the window had elapsed was closed by it",
        );
        assert_eq!(drain.open.load(Ordering::Acquire), 1);
        drop(kept);
        assert_eq!(drain.open.load(Ordering::Acquire), 0);
    }

    #[test]
    fn a_socket_holds_its_place_under_the_cap_until_it_drops() {
        let places = Arc::new(tokio::sync::Semaphore::new(1));
        let drain = Arc::new(Drain::default());
        let held = drain.socket(
            tokio::io::duplex(8).0,
            Arc::clone(&places)
                .try_acquire_owned()
                .expect("a free place"),
            SEND,
        );
        assert_eq!(places.available_permits(), 0);
        drop(held);
        assert_eq!(places.available_permits(), 1);
    }

    /// A socket over a pipe holding 8 bytes, its far end for the test to read.
    fn piped(
        drain: &Arc<Drain>,
    ) -> (
        DrainSocket<tokio::io::DuplexStream>,
        tokio::io::DuplexStream,
    ) {
        let (near, far) = tokio::io::duplex(8);
        let places = Arc::new(tokio::sync::Semaphore::new(1));
        let place = places.try_acquire_owned().expect("a free place");
        (drain.socket(near, place, SEND), far)
    }

    /// One write, polled once.
    fn write_once(
        socket: &mut DrainSocket<tokio::io::DuplexStream>,
        bytes: &[u8],
    ) -> Poll<IoResult<usize>> {
        let mut cx = Context::from_waker(std::task::Waker::noop());
        Pin::new(socket).poll_write(&mut cx, bytes)
    }

    /// A completed flush is no progress: a pipe, like TCP, always completes one.
    #[tokio::test(start_paused = true)]
    async fn a_write_the_peer_takes_nothing_of_fails_timed_out_at_the_send_deadline() {
        let drain = Arc::new(Drain::default());
        let (mut socket, _far) = piped(&drain);
        assert!(matches!(
            write_once(&mut socket, &[0; 8]),
            Poll::Ready(Ok(8))
        ));
        assert!(
            write_once(&mut socket, b"x").is_pending(),
            "the pipe is full"
        );

        tokio::time::advance(Duration::from_secs(1)).await;
        let mut cx = Context::from_waker(std::task::Waker::noop());
        assert!(Pin::new(&mut socket).poll_flush(&mut cx).is_ready());
        assert!(write_once(&mut socket, b"x").is_pending());
        tokio::time::advance(SEND - Duration::from_secs(1) - Duration::from_millis(1)).await;
        assert!(
            write_once(&mut socket, b"x").is_pending(),
            "inside the deadline"
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        match write_once(&mut socket, b"x") {
            Poll::Ready(Err(error)) => assert_eq!(error.kind(), std::io::ErrorKind::TimedOut),
            other => panic!("the stalled write fails at its deadline, got {other:?}"),
        }
    }

    /// A write that goes through clears the stall: the deadline runs from the
    /// next write that waits, never from the first.
    #[tokio::test(start_paused = true)]
    async fn progress_restarts_the_send_deadline() {
        use tokio::io::AsyncReadExt;
        let drain = Arc::new(Drain::default());
        let (mut socket, mut far) = piped(&drain);
        assert!(matches!(
            write_once(&mut socket, &[0; 8]),
            Poll::Ready(Ok(8))
        ));
        assert!(write_once(&mut socket, b"x").is_pending());

        tokio::time::advance(SEND - Duration::from_secs(1)).await;
        let mut taken = [0_u8; 1];
        far.read_exact(&mut taken)
            .await
            .expect("the peer takes a byte");
        assert!(matches!(write_once(&mut socket, b"x"), Poll::Ready(Ok(1))));
        assert!(write_once(&mut socket, b"x").is_pending(), "full again");

        tokio::time::advance(SEND - Duration::from_millis(1)).await;
        assert!(
            write_once(&mut socket, b"x").is_pending(),
            "the deadline restarted at the write that waited next",
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(matches!(write_once(&mut socket, b"x"), Poll::Ready(Err(_))));
    }

    #[tokio::test]
    async fn nothing_is_counted_as_cut_before_shutdown_is_asked_for() {
        let drain = Arc::new(Drain::default());
        drop(socket(&drain));
        assert_eq!(drain.closed_at_bound.load(Ordering::Acquire), 0);
        assert_eq!(drain.open.load(Ordering::Acquire), 0);
    }
}
