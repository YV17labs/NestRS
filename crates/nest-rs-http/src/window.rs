//! An HTTP/2 stream's send deadline: a frame its body handed over that the
//! peer's window has not taken within the send deadline resets the stream
//! (`RST_STREAM` `CANCEL`), and the connection's other streams go on.
//!
//! hyper polls a stream's body only once the frame before it found window
//! (`PipeToSendStream`), so a peer that reads its connection but grants a
//! stream no window parks it with no write waiting on the socket, where the
//! connection's send deadline is held (`drain.rs`). And hyper hands h2 a whole
//! frame as soon as the window has room for a byte of it, so a body hands it
//! at most a [`PIECE`](crate::phase::PIECE) at a time: the rest stays the
//! stream's, in flight and under this deadline, never in h2's buffer once the
//! stream's task is done and its connection counts idle.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::Poll;
use std::time::Duration;

use crate::stall::Stall;

tokio::task_local! {
    /// The running stream task's wait, for the body its request answers.
    static WAIT: Arc<WindowWait>;
}

/// Whether an HTTP/2 stream's body last handed over a frame that hyper has not
/// come back from: one the peer's window has not taken yet.
#[derive(Default)]
struct WindowWait {
    /// How many frames were handed over, shifted left once; the low bit is set
    /// while the last one waits.
    state: AtomicU64,
}

impl WindowWait {
    /// The frame waiting on the window, by its number.
    fn waiting(&self) -> Option<u64> {
        let state = self.state.load(Ordering::Relaxed);
        (state & 1 == 1).then_some(state >> 1)
    }
}

/// An HTTP/2 stream's window as its response body hands frames to it.
pub(crate) struct Window {
    wait: Arc<WindowWait>,
    handed: u64,
    waiting: bool,
}

impl Window {
    /// The running stream task's window: `Some` only on HTTP/2.
    pub(crate) fn current() -> Option<Self> {
        WAIT.try_with(|wait| Self {
            wait: Arc::clone(wait),
            handed: 0,
            waiting: false,
        })
        .ok()
    }

    /// hyper polls the body again: what it was handed has been taken.
    pub(crate) fn came_back(&mut self) {
        if std::mem::take(&mut self.waiting) {
            self.wait.state.store(self.handed << 1, Ordering::Relaxed);
        }
    }

    /// The body handed hyper a frame: it waits on the window until hyper comes
    /// back.
    pub(crate) fn handed(&mut self) {
        self.handed += 1;
        self.waiting = true;
        self.wait
            .state
            .store((self.handed << 1) | 1, Ordering::Relaxed);
    }
}

/// Run an HTTP/2 stream's task; `None` once a frame its body handed over has
/// waited on the peer's window past `send_timeout`, the task dropped with it.
///
/// The clock is read and the timer set only when the task waits on a frame
/// the window has not taken, never per request.
pub(crate) async fn within<F: Future>(stream: F, send_timeout: Duration) -> Option<F::Output> {
    let wait = Arc::new(WindowWait::default());
    let mut stream = pin!(WAIT.scope(Arc::clone(&wait), stream));
    let mut seen = None;
    let mut stall = Stall::new(send_timeout);
    std::future::poll_fn(move |cx| {
        if let Poll::Ready(output) = stream.as_mut().poll(cx) {
            return Poll::Ready(Some(output));
        }
        let Some(frame) = wait.waiting() else {
            return Poll::Pending;
        };
        if seen != Some(frame) {
            seen = Some(frame);
            stall.clear();
        }
        stall.poll_elapsed(cx).map(|()| None)
    })
    .await
}

#[cfg(test)]
mod tests {
    use tokio::time::Instant;

    use super::*;

    const SEND: Duration = Duration::from_secs(60);

    /// A stream task whose body hands one frame, then waits on the window
    /// for `parked`, then hands another and waits on it forever.
    async fn parked_twice(parked: Duration) {
        let mut window = Window::current().expect("inside a stream task");
        window.handed();
        tokio::time::sleep(parked).await;
        window.came_back();
        window.handed();
        std::future::pending::<()>().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_frame_the_window_never_takes_drops_the_stream_at_the_send_deadline() {
        let start = Instant::now();
        assert!(within(parked_twice(SEND / 2), SEND).await.is_none());
        assert_eq!(
            start.elapsed(),
            SEND / 2 + SEND,
            "counted from the frame that waits, the one taken before it no matter",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_stream_waiting_on_its_body_rather_than_the_window_is_never_cut() {
        let stream = async {
            let mut window = Window::current().expect("inside a stream task");
            window.handed();
            window.came_back();
            std::future::pending::<()>().await;
        };
        assert!(
            tokio::time::timeout(10 * SEND, within(stream, SEND))
                .await
                .is_err(),
            "a body slow to produce is no peer slow to read",
        );
    }

    #[test]
    fn outside_a_stream_task_there_is_no_window() {
        assert!(Window::current().is_none());
    }
}
