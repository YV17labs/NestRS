//! The transport's way down as the connections it serves see it: the instant
//! shutdown is asked for, the instant the window closes, and what is still open
//! at the second.
//!
//! The edge reads the first two. A response whose body has no end of its own
//! ([`OpenEndedBody`](crate::OpenEndedBody)) is ended at the signal, and a body
//! dropped unfinished once the window has closed was cut by it — both say so on
//! their `http.request` line, which is why the edge holds the drain rather than
//! the transport alone.
//!
//! poem closes every connection it still serves once the window handed to
//! `run_with_graceful_shutdown` elapses, and keeps its count of them private,
//! so it cannot say how many that was. The transport therefore keeps its own
//! tally, one level down: every socket the listener accepts counts as open
//! until it is dropped. Counting sockets rather than requests is also what lets
//! it see the connections poem does not: an upgraded one (a WebSocket) leaves
//! poem's count at the upgrade, and the window does not close it. A gateway's
//! socket is carried by its mount's [`DetachedWork`](crate::DetachedWork), which
//! closes it; one a hand-built endpoint upgraded is the developer's own, and
//! this count is what says it outlived the transport.

use std::io::IoSlice;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use poem::http::uri::Scheme;
use poem::listener::{Acceptor, Listener};
use poem::web::{LocalAddr, RemoteAddr};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, Result as IoResult};
use tokio::time::Instant;
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
    /// `listener`, with every socket it accepts counted by this drain.
    pub(crate) fn track<L: Listener>(self: &Arc<Self>, listener: L) -> DrainListener<L> {
        DrainListener {
            inner: listener,
            drain: Arc::clone(self),
        }
    }

    /// Shutdown was asked for: the window starts now.
    ///
    /// Called from the signal future poem awaits, so it runs before poem
    /// starts its own clock — every socket poem closes at the bound is
    /// therefore dropped at or after the instant recorded here, and one that
    /// closed on its own before it is not counted as closed by it.
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

/// A poem [`Listener`] whose acceptor counts every socket it hands out.
pub(crate) struct DrainListener<L> {
    inner: L,
    drain: Arc<Drain>,
}

impl<L: Listener> Listener for DrainListener<L> {
    type Acceptor = DrainAcceptor<L::Acceptor>;

    async fn into_acceptor(self) -> IoResult<Self::Acceptor> {
        Ok(DrainAcceptor {
            inner: self.inner.into_acceptor().await?,
            drain: self.drain,
        })
    }
}

/// The acceptor of a [`DrainListener`].
pub(crate) struct DrainAcceptor<A> {
    inner: A,
    drain: Arc<Drain>,
}

impl<A: Acceptor> Acceptor for DrainAcceptor<A> {
    type Io = DrainSocket<A::Io>;

    fn local_addr(&self) -> Vec<LocalAddr> {
        self.inner.local_addr()
    }

    async fn accept(&mut self) -> IoResult<(Self::Io, LocalAddr, RemoteAddr, Scheme)> {
        let (io, local, remote, scheme) = self.inner.accept().await?;
        Ok((DrainSocket::new(io, &self.drain), local, remote, scheme))
    }
}

/// One accepted socket, counted open until it is dropped. Every call is
/// forwarded untouched — vectored writes included, which hyper uses when the
/// socket offers them.
pub(crate) struct DrainSocket<Io> {
    inner: Io,
    drain: Arc<Drain>,
}

impl<Io> DrainSocket<Io> {
    fn new(inner: Io, drain: &Arc<Drain>) -> Self {
        drain.opened();
        Self {
            inner,
            drain: Arc::clone(drain),
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
    use super::*;

    fn socket(drain: &Arc<Drain>) -> DrainSocket<tokio::io::DuplexStream> {
        DrainSocket::new(tokio::io::duplex(8).0, drain)
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

    #[tokio::test]
    async fn nothing_is_counted_as_cut_before_shutdown_is_asked_for() {
        let drain = Arc::new(Drain::default());
        drop(socket(&drain));
        assert_eq!(drain.closed_at_bound.load(Ordering::Acquire), 0);
        assert_eq!(drain.open.load(Ordering::Acquire), 0);
    }
}
