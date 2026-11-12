//! [`Signal`], a wake-up one task raises for the one task polling it — the
//! Ping a socket's reader owes its writer, a frame waiting to be written.

use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use futures_util::task::AtomicWaker;

/// A wake-up one task raises and the one task polling it takes, at most one
/// pending at a time. Lock-free, so a writer polling it beside every frame it
/// writes pays a few atomic operations for it, where a `tokio::sync::Notify`
/// takes its waiters' lock to register each wait and again to drop it.
#[derive(Default)]
pub struct Signal {
    raised: AtomicBool,
    waker: AtomicWaker,
}

impl Signal {
    /// Raise it, waking the task polling it; raised again before it is taken,
    /// it is taken once.
    pub fn raise(&self) {
        self.raised.store(true, Ordering::Release);
        self.waker.wake();
    }

    /// Ready once raised, taking it; one task polls it at a time.
    pub fn poll_taken(&self, cx: &mut Context<'_>) -> Poll<()> {
        if self.raised.swap(false, Ordering::AcqRel) {
            return Poll::Ready(());
        }
        self.waker.register(cx.waker());
        // Again after registering: a raise between the two would wake no one.
        if self.raised.swap(false, Ordering::AcqRel) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    /// [`poll_taken`](Self::poll_taken) as a future, cancel-safe: dropped
    /// unfinished, it leaves the signal as it found it.
    pub async fn taken(&self) {
        std::future::poll_fn(|cx| self.poll_taken(cx)).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn a_raise_before_the_wait_is_taken_once() {
        let signal = Signal::default();
        signal.raise();
        signal.raise();

        signal.taken().await;
        let again = tokio::time::timeout(Duration::from_millis(10), signal.taken()).await;
        assert!(again.is_err(), "two raises before the wait are one");
    }

    #[tokio::test]
    async fn a_raise_wakes_the_task_waiting_on_it() {
        let signal = Arc::new(Signal::default());
        let waiting = tokio::spawn({
            let signal = Arc::clone(&signal);
            async move { signal.taken().await }
        });
        tokio::task::yield_now().await;

        signal.raise();
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("the waiting task is woken")
            .expect("the waiting task never panics");
    }

    #[test]
    fn a_raise_after_a_pending_poll_is_taken_once() {
        let signal = Signal::default();
        let mut cx = Context::from_waker(std::task::Waker::noop());
        assert!(signal.poll_taken(&mut cx).is_pending());

        signal.raise();
        assert!(signal.poll_taken(&mut cx).is_ready());
        assert!(signal.poll_taken(&mut cx).is_pending(), "taken, it is down");
    }
}
