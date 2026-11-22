//! The bulkhead a policy keeps: how many calls run inside it at once, how long
//! a call waits for a slot before it is refused, and how many calls wait for
//! one at once.

use std::time::Duration;

use tokio::sync::{Semaphore, SemaphorePermit};
use tokio::time::Instant;

/// A bound on the calls running inside one policy at once.
pub(crate) struct Bulkhead {
    slots: Semaphore,
    wait: Duration,
    /// The places a call finding no free slot waits in, when their number is
    /// bounded and a call waits at all.
    waiters: Option<Waiters>,
}

/// The places to wait for a slot in, `most` of them.
struct Waiters {
    places: Semaphore,
    most: usize,
}

/// Why the bulkhead took a call into no slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Full {
    /// Every slot stayed taken for as long as the call waits for one: its
    /// wait, what is left of the unit's deadline when that is sooner, or none.
    Slots,
    /// Every slot was taken, and every place to wait for one: as many calls
    /// as may wait were waiting, this many.
    Waiters(usize),
}

impl Full {
    /// How many calls were waiting, when that is why the call was refused.
    pub(crate) const fn waiters(self) -> Option<usize> {
        match self {
            Self::Slots => None,
            Self::Waiters(most) => Some(most),
        }
    }
}

impl Bulkhead {
    /// A bulkhead of `slots`, a call finding none free waiting up to `wait`
    /// for one, and at most `waiters` calls waiting at once when they are
    /// bounded. With a wait of none no call waits, so the bound builds nothing.
    pub(crate) fn new(slots: usize, wait: Duration, waiters: Option<usize>) -> Self {
        Self {
            slots: Semaphore::new(slots),
            wait,
            waiters: waiters.filter(|_| !wait.is_zero()).map(|most| Waiters {
                places: Semaphore::new(most),
                most,
            }),
        }
    }

    /// A slot free now, held until the slot is dropped, or `None` while every
    /// slot is taken.
    pub(crate) fn try_enter(&self) -> Option<SemaphorePermit<'_>> {
        self.slots.try_acquire().ok()
    }

    /// A slot, held until the slot is dropped, or why none was taken: every
    /// place to wait for one taken, at once, or no slot freed within the wait
    /// — the bulkhead's own, or what is left before `deadline` when that is
    /// sooner; at once for a wait of none.
    pub(crate) async fn enter(
        &self,
        deadline: Option<Instant>,
    ) -> Result<SemaphorePermit<'_>, Full> {
        if let Some(free) = self.try_enter() {
            return Ok(free);
        }
        let wait = deadline.map_or(self.wait, |deadline| {
            self.wait
                .min(deadline.saturating_duration_since(Instant::now()))
        });
        if wait.is_zero() {
            return Err(Full::Slots);
        }
        // A place is taken in one step, never counted then checked, so no more
        // than `most` calls wait; it is given up with the wait, however it ends.
        let _place = self
            .waiters
            .as_ref()
            .map(|waiters| {
                waiters
                    .places
                    .try_acquire()
                    .ok()
                    .ok_or(Full::Waiters(waiters.most))
            })
            .transpose()?;
        // The semaphore is never closed, so its one refusal is no slot in time.
        tokio::time::timeout(wait, self.slots.acquire())
            .await
            .ok()
            .and_then(Result::ok)
            .ok_or(Full::Slots)
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::{Pin, pin};
    use std::sync::Arc;
    use std::task::Poll;

    use super::*;

    /// Whether `entering`, polled once more, still waits for a slot.
    async fn waits<F: Future>(mut entering: Pin<&mut F>) -> bool {
        std::future::poll_fn(|cx| Poll::Ready(entering.as_mut().poll(cx).is_pending())).await
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_bulkhead_with_no_wait_refuses_at_once() {
        let bulkhead = Bulkhead::new(1, Duration::ZERO, None);
        let held = bulkhead.enter(None).await.expect("a free slot");
        assert_eq!(bulkhead.enter(None).await.err(), Some(Full::Slots));
        drop(held);
        assert!(bulkhead.enter(None).await.is_ok(), "the slot came back");
    }

    /// The wait for a slot never outlasts the unit the call runs in.
    #[tokio::test(start_paused = true)]
    async fn the_wait_for_a_slot_ends_at_the_units_deadline_when_that_is_sooner() {
        let bulkhead = Bulkhead::new(1, Duration::from_secs(60), None);
        let held = bulkhead.enter(None).await.expect("a free slot");
        let started = Instant::now();
        let deadline = started + Duration::from_millis(200);
        assert_eq!(
            bulkhead.enter(Some(deadline)).await.err(),
            Some(Full::Slots)
        );
        assert_eq!(started.elapsed(), Duration::from_millis(200));
        assert_eq!(
            bulkhead.enter(Some(Instant::now())).await.err(),
            Some(Full::Slots),
            "a unit out of time waits for nothing"
        );
        assert_eq!(started.elapsed(), Duration::from_millis(200));
        drop(held);
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_waits_for_a_slot_up_to_its_wait() {
        let bulkhead = Bulkhead::new(1, Duration::from_millis(500), None);
        let held = bulkhead.enter(None).await.expect("a free slot");
        let started = tokio::time::Instant::now();
        assert_eq!(
            bulkhead.enter(None).await.err(),
            Some(Full::Slots),
            "no slot freed in time"
        );
        assert_eq!(started.elapsed(), Duration::from_millis(500));

        let waiting = bulkhead.enter(None);
        let freeing = async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            drop(held);
        };
        let (entered, ()) = tokio::join!(waiting, freeing);
        assert!(entered.is_ok(), "the slot freed inside the wait");
    }

    /// A bound on the waiters means nothing where no call waits, as a wait
    /// means nothing without slots: with a wait of none it builds no place.
    #[test]
    fn a_bound_on_the_waiters_builds_its_places_only_beside_a_wait() {
        let wait = Duration::from_secs(1);
        assert!(Bulkhead::new(1, Duration::ZERO, Some(4)).waiters.is_none());
        assert!(Bulkhead::new(1, wait, None).waiters.is_none());
        let bounded = Bulkhead::new(1, wait, Some(4));
        assert_eq!(
            bounded.waiters.as_ref().map(|waiters| waiters.most),
            Some(4)
        );
    }

    /// A waiter its caller drops gives its place up: the next call finding
    /// the slot taken waits in it, rather than being refused at once.
    #[tokio::test(start_paused = true)]
    async fn a_waiter_dropped_mid_wait_frees_its_place() {
        let bulkhead = Bulkhead::new(1, Duration::from_secs(60), Some(1));
        let held = bulkhead.enter(None).await.expect("a free slot");
        let mut dropped = Box::pin(bulkhead.enter(None));
        assert!(waits(dropped.as_mut()).await, "it took the one place");
        assert_eq!(
            bulkhead.enter(None).await.err(),
            Some(Full::Waiters(1)),
            "no place is left"
        );
        drop(dropped);

        let mut next = pin!(bulkhead.enter(None));
        assert!(waits(next.as_mut()).await, "it waits in the place freed");
        drop(held);
        assert!(next.await.is_ok());
    }

    /// A waiter whose wait ran out gives its place up: the next call waits its
    /// whole wait too, rather than being refused at once.
    #[tokio::test(start_paused = true)]
    async fn a_waiter_whose_wait_ran_out_frees_its_place() {
        let bulkhead = Bulkhead::new(1, Duration::from_millis(500), Some(1));
        let _held = bulkhead.enter(None).await.expect("a free slot");
        let started = Instant::now();
        for waited in [Duration::from_millis(500), Duration::from_secs(1)] {
            assert_eq!(bulkhead.enter(None).await.err(), Some(Full::Slots));
            assert_eq!(started.elapsed(), waited, "it waited in the one place");
        }
    }

    /// A waiter handed its slot gives its place up: the next call waits for
    /// the slot it holds, rather than being refused at once.
    #[tokio::test(start_paused = true)]
    async fn a_waiter_that_gets_its_slot_frees_its_place() {
        let bulkhead = Bulkhead::new(1, Duration::from_secs(60), Some(1));
        let held = bulkhead.enter(None).await.expect("a free slot");
        let (entered, ()) = tokio::join!(bulkhead.enter(None), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(held);
        });
        let entered = entered.expect("the slot freed for it");

        let mut next = pin!(bulkhead.enter(None));
        assert!(waits(next.as_mut()).await, "it waits in the place freed");
        drop(entered);
        assert!(next.await.is_ok());
    }

    /// Callers racing on many threads take no more places than there are: a
    /// place is reserved in one step, so exactly the bound waits and every
    /// other caller is refused while the slot is still held.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn callers_racing_on_many_threads_leave_exactly_the_bound_waiting() {
        const WAITERS: usize = 4;
        const CALLERS: usize = 64;
        let bulkhead = Arc::new(Bulkhead::new(1, Duration::from_secs(60), Some(WAITERS)));
        let held = bulkhead.enter(None).await.expect("a free slot");
        let start = Arc::new(tokio::sync::Barrier::new(CALLERS));
        let callers: Vec<_> = (0..CALLERS)
            .map(|_| {
                let (bulkhead, start) = (Arc::clone(&bulkhead), Arc::clone(&start));
                tokio::spawn(async move {
                    start.wait().await;
                    bulkhead.enter(None).await.map(drop)
                })
            })
            .collect();
        nest_rs_testing::wait_until(Duration::from_secs(10), || {
            callers.iter().filter(|caller| caller.is_finished()).count() >= CALLERS - WAITERS
        })
        .await;
        assert_eq!(
            callers
                .iter()
                .filter(|caller| !caller.is_finished())
                .count(),
            WAITERS,
            "exactly the bound waits while the slot is held"
        );
        drop(held);

        let mut answered = Vec::new();
        for caller in callers {
            answered.push(caller.await.expect("the caller's task").err());
        }
        assert_eq!(
            answered.iter().filter(|refused| refused.is_none()).count(),
            WAITERS,
            "each waiter took the slot in turn"
        );
        assert_eq!(
            answered
                .iter()
                .filter(|refused| **refused == Some(Full::Waiters(WAITERS)))
                .count(),
            CALLERS - WAITERS,
            "every other caller was refused for want of a place"
        );
    }
}
