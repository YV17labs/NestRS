//! [`Bulkhead`] — how many calls to one dependency run at once.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// At most [`max_concurrent`](Self::max_concurrent) calls through the policy at
/// once; a call past them queues for a slot at most
/// [`max_wait`](Self::max_wait) (none by default) and is refused
/// `BULKHEAD_FULL` past it. It keeps a slow dependency from taking every
/// worker the process has.
///
/// ```
/// use std::num::NonZeroU32;
/// use std::time::Duration;
/// use nest_rs_resilience::Bulkhead;
///
/// let bulkhead = Bulkhead::new(NonZeroU32::new(16).expect("non-zero"))
///     .with_max_wait(Duration::from_millis(100));
/// assert_eq!(bulkhead.max_concurrent().get(), 16);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bulkhead {
    max_concurrent: NonZeroU32,
    max_wait: Duration,
}

impl Bulkhead {
    /// At most `max_concurrent` calls at once, refusing the next at once.
    pub const fn new(max_concurrent: NonZeroU32) -> Self {
        Self {
            max_concurrent,
            max_wait: Duration::ZERO,
        }
    }

    /// Lets a call past the slots queue `max_wait` for one.
    pub const fn with_max_wait(mut self, max_wait: Duration) -> Self {
        self.max_wait = max_wait;
        self
    }

    /// How many calls run at once.
    pub const fn max_concurrent(&self) -> NonZeroU32 {
        self.max_concurrent
    }

    /// How long a call may queue for a slot.
    pub const fn max_wait(&self) -> Duration {
        self.max_wait
    }
}

/// One bulkhead's live slots.
#[derive(Debug)]
pub(crate) struct Slots {
    spec: Bulkhead,
    semaphore: Arc<Semaphore>,
}

impl Slots {
    pub(crate) fn new(spec: Bulkhead) -> Self {
        Self {
            spec,
            semaphore: Arc::new(Semaphore::new(spec.max_concurrent.get() as usize)),
        }
    }

    /// A slot, waiting at most the bulkhead's `max_wait` — and never past
    /// `bound`, the call's remaining deadline.
    pub(crate) async fn acquire(&self, bound: Option<Duration>) -> Option<OwnedSemaphorePermit> {
        if let Ok(permit) = self.semaphore.clone().try_acquire_owned() {
            return Some(permit);
        }
        let wait = bound.map_or(self.spec.max_wait, |bound| bound.min(self.spec.max_wait));
        if wait.is_zero() {
            return None;
        }
        tokio::time::timeout(wait, self.semaphore.clone().acquire_owned())
            .await
            .ok()
            .and_then(Result::ok)
    }

    /// How many slots are free right now.
    pub(crate) fn available(&self) -> usize {
        self.semaphore.available_permits()
    }
}
