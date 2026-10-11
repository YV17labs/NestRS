//! [`Capped`]: the one wrapper the edge puts around every request body that is
//! not empty, so whoever reads it — an extractor, a guard, a global pipe, a
//! self-mount — reads it under the transport's cap and its gap bound, after
//! routing and the guards rather than at the edge.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use http_body_util::combinators::BoxBody;
use tokio::time::{Instant, Sleep};

use crate::error::BodyError;

/// Where a read through [`Capped`] stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Overrun {
    /// More bytes arrived than the cap, in bytes.
    Cap(usize),
    /// No frame arrived within the gap bound.
    Gap(Duration),
}

impl Overrun {
    /// The error this stop is, as the edge answers it.
    pub(crate) fn error(self) -> BodyError {
        match self {
            Self::Cap(limit) => BodyError::TooLarge { limit },
            Self::Gap(after) => BodyError::Stalled { after },
        }
    }

    /// The error the reader's poll fails with.
    fn io(self) -> io::Error {
        let kind = match self {
            Self::Cap(_) => io::ErrorKind::Other,
            Self::Gap(_) => io::ErrorKind::TimedOut,
        };
        io::Error::new(kind, self.error())
    }
}

/// Where a [`Capped`] body stopped, set once by the body and read by the edge
/// once the inner tree has returned: the edge answers it whatever the reader
/// made of the failed read.
pub(crate) type Verdict = Arc<OnceLock<Overrun>>;

/// A request body counted against the cap where its declared length cannot be
/// trusted, and failed when no frame arrives within the gap bound once a
/// reader waits on one.
pub(crate) struct Capped {
    inner: BoxBody<Bytes, io::Error>,
    /// `None` where hyper's length decoder already holds the body to a declared
    /// length under the cap.
    limit: Option<usize>,
    seen: usize,
    gap: Duration,
    /// Allocated on the first `Pending`, so a body already buffered pays for no
    /// timer.
    timer: Option<Pin<Box<Sleep>>>,
    /// No frame has arrived since the timer was set.
    armed: bool,
    verdict: Verdict,
}

impl Capped {
    /// `body`, counted against `limit` when one is given, its gaps bounded by
    /// `gap`; where it stops is told to `verdict`.
    pub(crate) fn new(
        body: BoxBody<Bytes, io::Error>,
        limit: Option<usize>,
        gap: Duration,
        verdict: Verdict,
    ) -> Self {
        Self {
            inner: body,
            limit,
            seen: 0,
            gap,
            timer: None,
            armed: false,
            verdict,
        }
    }

    /// The inner body has nothing yet: wait on the gap bound, set when the
    /// last frame was taken.
    fn wait(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        let gap = self.gap;
        let timer = self
            .timer
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(gap)));
        if !self.armed {
            timer.as_mut().reset(Instant::now() + gap);
            self.armed = true;
        }
        match timer.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Some(Err(self.stop(Overrun::Gap(gap))))),
            Poll::Pending => Poll::Pending,
        }
    }

    /// Record `overrun` for the edge, and fail the read with it.
    fn stop(&self, overrun: Overrun) -> io::Error {
        self.verdict.get_or_init(|| overrun);
        overrun.io()
    }
}

impl Body for Capped {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        let this = self.get_mut();
        // A stopped body ends: a reader that polls on past the error must not
        // spin on it, and what it made of the rest, the edge's answer overrides.
        if this.verdict.get().is_some() {
            return Poll::Ready(None);
        }
        match Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                this.armed = false;
                if let (Some(limit), Some(data)) = (this.limit, frame.data_ref()) {
                    this.seen = this.seen.saturating_add(data.len());
                    if this.seen > limit {
                        return Poll::Ready(Some(Err(this.stop(Overrun::Cap(limit)))));
                    }
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Pending => this.wait(cx),
            ended => ended,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.verdict.get().is_some() || self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use futures_util::StreamExt;
    use http_body_util::{BodyExt, Full, StreamBody};

    use super::*;

    /// A body whose frames are each `Some(bytes)`, or `None` for a poll that
    /// finds nothing and wakes its reader at once.
    fn scripted(
        frames: impl IntoIterator<Item = Option<&'static [u8]>>,
    ) -> BoxBody<Bytes, io::Error> {
        let mut frames: VecDeque<_> = frames.into_iter().collect();
        BoxBody::new(StreamBody::new(futures_util::stream::poll_fn(
            move |cx| match frames.pop_front() {
                Some(Some(bytes)) => Poll::Ready(Some(Ok::<_, io::Error>(Frame::data(
                    Bytes::from_static(bytes),
                )))),
                Some(None) => {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
                None => Poll::Ready(None),
            },
        )))
    }

    fn full(bytes: &'static [u8]) -> BoxBody<Bytes, io::Error> {
        BoxBody::new(Full::new(Bytes::from_static(bytes)).map_err(|never| match never {}))
    }

    /// Every frame of `body`, or the error that ended it.
    async fn read(mut body: Capped) -> (Capped, Result<Vec<u8>, io::Error>) {
        let mut read = Vec::new();
        while let Some(frame) = body.frame().await {
            match frame {
                Ok(frame) => {
                    if let Some(data) = frame.data_ref() {
                        read.extend_from_slice(data);
                    }
                }
                Err(error) => return (body, Err(error)),
            }
        }
        (body, Ok(read))
    }

    #[tokio::test]
    async fn the_gap_timer_arms_only_on_pending() {
        let (ready, read_whole) = read(Capped::new(
            full(b"all there"),
            None,
            Duration::from_secs(30),
            Verdict::default(),
        ))
        .await;
        assert_eq!(read_whole.expect("read"), b"all there");
        assert!(
            ready.timer.is_none(),
            "a body that never pends allocates no timer"
        );

        let (pended, read_whole) = read(Capped::new(
            scripted([Some(&b"a"[..]), None, Some(&b"b"[..])]),
            None,
            Duration::from_secs(30),
            Verdict::default(),
        ))
        .await;
        assert_eq!(read_whole.expect("read"), b"ab");
        assert!(pended.timer.is_some(), "a pending read arms the timer");
        assert!(!pended.armed, "the frame after the wait clears it");
    }

    #[tokio::test(start_paused = true)]
    async fn a_body_with_no_frame_within_the_gap_fails_its_read_timed_out() {
        let verdict = Verdict::default();
        let body = BoxBody::new(StreamBody::new(
            futures_util::stream::iter([Ok::<_, io::Error>(Frame::data(Bytes::from_static(
                b"part",
            )))])
            .chain(futures_util::stream::pending()),
        ));
        let gap = Duration::from_secs(30);
        let (_, read_whole) = read(Capped::new(body, None, gap, Arc::clone(&verdict))).await;

        let error = read_whole.expect_err("the body stalled");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(matches!(
            error.get_ref().and_then(|e| e.downcast_ref::<BodyError>()),
            Some(BodyError::Stalled { after }) if *after == gap
        ));
        assert_eq!(verdict.get(), Some(&Overrun::Gap(gap)));
    }

    #[tokio::test(start_paused = true)]
    async fn each_frame_restarts_the_gap() {
        let body = BoxBody::new(StreamBody::new(futures_util::stream::iter(0..4).then(
            |_| async {
                tokio::time::sleep(Duration::from_secs(20)).await;
                Ok::<_, io::Error>(Frame::data(Bytes::from_static(b"x")))
            },
        )));
        let (_, read_whole) = read(Capped::new(
            body,
            None,
            Duration::from_secs(30),
            Verdict::default(),
        ))
        .await;
        assert_eq!(
            read_whole.expect("a body slower in all than one gap, never stalled"),
            b"xxxx"
        );
    }

    #[tokio::test]
    async fn past_the_cap_the_read_fails_then_ends() {
        let verdict = Verdict::default();
        let mut body = Capped::new(
            scripted([Some(&b"1234"[..]), Some(&b"5678"[..]), Some(&b"9"[..])]),
            Some(8),
            Duration::from_secs(30),
            Arc::clone(&verdict),
        );
        assert!(body.frame().await.is_some_and(|f| f.is_ok()));
        assert!(
            body.frame().await.is_some_and(|f| f.is_ok()),
            "8 bytes is the cap, not past it"
        );
        let error = body
            .frame()
            .await
            .expect("a frame")
            .expect_err("the ninth byte is past the cap");
        assert!(matches!(
            error.get_ref().and_then(|e| e.downcast_ref::<BodyError>()),
            Some(BodyError::TooLarge { limit: 8 })
        ));
        assert_eq!(verdict.get(), Some(&Overrun::Cap(8)));
        assert!(
            body.frame().await.is_none(),
            "a read after the stop ends rather than going on or failing for ever"
        );
        assert!(body.is_end_stream());
    }

    #[tokio::test]
    async fn an_uncounted_body_is_left_to_its_length_and_keeps_its_size() {
        let body = Capped::new(
            full(b"0123456789"),
            None,
            Duration::from_secs(30),
            Verdict::default(),
        );
        assert_eq!(
            body.size_hint().exact(),
            Some(10),
            "a declared length survives the wrap"
        );
        let (_, read_whole) = read(body).await;
        assert_eq!(read_whole.expect("no cap counted").len(), 10);
    }
}
