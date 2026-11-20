//! The bulkhead a policy keeps: how many calls run inside it at once, and how
//! long a call waits for a slot before it is refused.

use std::time::Duration;

use tokio::sync::{Semaphore, SemaphorePermit};

/// A bound on the calls running inside one policy at once.
pub(crate) struct Bulkhead {
    slots: Semaphore,
    wait: Duration,
}

impl Bulkhead {
    pub(crate) fn new(slots: usize, wait: Duration) -> Self {
        Self {
            slots: Semaphore::new(slots),
            wait,
        }
    }

    /// A slot, held until the slot is dropped, or `None` once the wait for one
    /// ran out — at once for a wait of none.
    pub(crate) async fn enter(&self) -> Option<SemaphorePermit<'_>> {
        if let Ok(free) = self.slots.try_acquire() {
            return Some(free);
        }
        if self.wait.is_zero() {
            return None;
        }
        // The semaphore is never closed, so its one refusal is no slot in time.
        tokio::time::timeout(self.wait, self.slots.acquire())
            .await
            .ok()
            .and_then(Result::ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_full_bulkhead_with_no_wait_refuses_at_once() {
        let bulkhead = Bulkhead::new(1, Duration::ZERO);
        let held = bulkhead.enter().await.expect("a free slot");
        assert!(bulkhead.enter().await.is_none());
        drop(held);
        assert!(bulkhead.enter().await.is_some(), "the slot came back");
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_waits_for_a_slot_up_to_its_wait() {
        let bulkhead = Bulkhead::new(1, Duration::from_millis(500));
        let held = bulkhead.enter().await.expect("a free slot");
        let started = tokio::time::Instant::now();
        assert!(bulkhead.enter().await.is_none(), "no slot freed in time");
        assert_eq!(started.elapsed(), Duration::from_millis(500));

        let waiting = bulkhead.enter();
        let freeing = async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            drop(held);
        };
        let (entered, ()) = tokio::join!(waiting, freeing);
        assert!(entered.is_some(), "the slot freed inside the wait");
    }
}
