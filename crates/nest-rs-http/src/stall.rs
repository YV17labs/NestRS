//! [`Stall`]: the bound on one wait for a peer — a write it takes nothing of,
//! a frame its HTTP/2 window holds, a request body's next frame.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::time::{Instant, Sleep};

/// A bound on one wait, counted from the first poll that finds it waiting and
/// cleared by the progress that ends it. The clock is read and the timer set
/// only when something waits; the timer is allocated on the first wait and
/// kept for the next.
pub(crate) struct Stall {
    bound: Duration,
    timer: Option<Pin<Box<Sleep>>>,
    /// A wait is under way, and `timer` holds its deadline.
    waiting: bool,
}

impl Stall {
    /// No wait yet; each one held to `bound`.
    pub(crate) const fn new(bound: Duration) -> Self {
        Self {
            bound,
            timer: None,
            waiting: false,
        }
    }

    /// What each wait is held to.
    pub(crate) const fn bound(&self) -> Duration {
        self.bound
    }

    /// Progress ended the wait: the next one counts from its own start.
    pub(crate) fn clear(&mut self) {
        self.waiting = false;
    }

    /// Still waiting: `Ready` once this wait has lasted the bound, counted
    /// from its first poll.
    pub(crate) fn poll_elapsed(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let timer = match &mut self.timer {
            Some(timer) if self.waiting => timer,
            Some(timer) => {
                timer.as_mut().reset(Instant::now() + self.bound);
                timer
            }
            None => self.timer.insert(Box::pin(tokio::time::sleep(self.bound))),
        };
        self.waiting = true;
        timer.as_mut().poll(cx)
    }
}

#[cfg(test)]
impl Stall {
    /// Whether a wait ever set the timer.
    pub(crate) fn has_timer(&self) -> bool {
        self.timer.is_some()
    }

    /// Whether a wait is under way.
    pub(crate) fn is_waiting(&self) -> bool {
        self.waiting
    }
}
