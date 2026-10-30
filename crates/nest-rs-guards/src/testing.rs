//! Doubles for this crate's unit tests: a guard's future driven to its end on
//! the test's thread.

use std::future::Future;
use std::task::{Context, Poll, Waker};

/// Build `fut` and poll it until it is ready, on this thread.
pub(crate) fn run<F: Future>(fut: impl FnOnce() -> F) -> F::Output {
    let mut fut = std::pin::pin!(fut());
    let mut poll = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = fut.as_mut().poll(&mut poll) {
            return output;
        }
    }
}
