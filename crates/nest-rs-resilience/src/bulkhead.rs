//! The bulkhead a policy keeps: how many calls run inside it at once, how long
//! a call waits for a slot before it is refused, and how many calls wait for
//! one at once.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::time::Instant;

/// A bound on the calls running inside one policy at once.
pub(crate) struct Bulkhead {
    slots: Semaphore,
    /// The calls holding a slot, in the word's high half, and the ones
    /// waiting for one, in its low half: one word, so a waiter handed its
    /// slot moves from one count to the other in one step, and a call is in
    /// exactly one at every instant. A half holds four billion calls, more
    /// futures than a process has memory for.
    calls: AtomicU64,
    /// How long a call finding no free slot waits for one, `None` for no wait.
    wait: Option<Duration>,
    /// The places a call finding no free slot waits in, when their number is
    /// bounded and a call waits at all.
    waiters: Option<Waiters>,
}

/// One call holding a slot, in [`Bulkhead::calls`].
const RUNNING: u64 = 1 << u32::BITS;

/// One call waiting for a slot, in [`Bulkhead::calls`].
const WAITING: u64 = 1;

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

/// A slot one call holds, counted among the calls running from the moment
/// its own future took it until it is dropped, which frees it.
#[must_use = "a slot is freed as it is dropped"]
pub(crate) struct Slot<'b>(&'b Bulkhead);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// A slot its run took, held past the run by what the run answered — a
/// response whose body is still to read — with no borrow of the bulkhead. It
/// is freed through [`Bulkhead::free`] alone, never by its drop, so what holds
/// it frees it at its own.
#[cfg(feature = "reqwest")]
#[must_use = "a held slot is freed through `Bulkhead::free` alone"]
pub(crate) struct HeldSlot(());

impl Bulkhead {
    /// `slot`, one of this bulkhead's, held with no borrow of it: its call
    /// stays counted among the ones running.
    #[cfg(feature = "reqwest")]
    pub(crate) fn hold(&self, slot: Slot<'_>) -> HeldSlot {
        debug_assert!(
            std::ptr::eq(slot.0, self),
            "a slot is held for the bulkhead it was taken from"
        );
        std::mem::forget(slot);
        HeldSlot(())
    }

    /// The slot `held` took, free again.
    #[cfg(feature = "reqwest")]
    pub(crate) fn free(&self, held: HeldSlot) {
        let HeldSlot(()) = held;
        self.release();
    }

    /// A bulkhead of `slots`, a call finding none free waiting up to `wait`
    /// for one, and at most `waiters` calls waiting at once when they are
    /// bounded. With no wait no call waits, so the bound builds nothing.
    pub(crate) fn new(slots: usize, wait: Option<Duration>, waiters: Option<usize>) -> Self {
        Self {
            slots: Semaphore::new(slots),
            calls: AtomicU64::new(0),
            wait,
            waiters: waiters.filter(|_| wait.is_some()).map(|most| Waiters {
                places: Semaphore::new(most),
                most,
            }),
        }
    }

    /// How many calls hold a slot now, one held past its run included. A
    /// slot freed for a call waiting counts once that call's own future has
    /// taken it: until then the call is read as [`waiting`](Self::waiting).
    pub(crate) fn running(&self) -> usize {
        half(self.calls.load(Ordering::Relaxed) >> u32::BITS)
    }

    /// How many calls wait for a slot now.
    pub(crate) fn waiting(&self) -> usize {
        half(self.calls.load(Ordering::Relaxed) & (RUNNING - 1))
    }

    /// The longest a call finding no free slot waits for one: none, with no wait.
    pub(crate) fn longest_wait(&self) -> Duration {
        self.wait.unwrap_or_default()
    }

    /// A slot free now, held until the slot is dropped, or `None` while every
    /// slot is taken, one freed for a call waiting included.
    pub(crate) fn try_enter(&self) -> Option<Slot<'_>> {
        self.slots.try_acquire().ok()?.forget();
        self.calls.fetch_add(RUNNING, Ordering::Relaxed);
        Some(Slot(self))
    }

    /// A slot, held until the slot is dropped, or why none was taken: every
    /// place to wait for one taken, at once, or no slot freed within the wait
    /// — the bulkhead's own, or what is left before `deadline` when that is
    /// sooner; at once with no wait, or none left before `deadline`.
    pub(crate) async fn enter(&self, deadline: Option<Instant>) -> Result<Slot<'_>, Full> {
        if let Some(free) = self.try_enter() {
            return Ok(free);
        }
        let Some(wait) = self.wait else {
            return Err(Full::Slots);
        };
        let wait = deadline.map_or(wait, |deadline| {
            wait.min(deadline.saturating_duration_since(Instant::now()))
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
        let waiting = Waiting::among(self);
        // The semaphore is never closed, so its one refusal is no slot in time.
        match tokio::time::timeout(wait, self.slots.acquire()).await {
            Ok(Ok(permit)) => {
                permit.forget();
                Ok(waiting.handed())
            }
            _ => Err(Full::Slots),
        }
    }

    /// One slot free again, its call counted out first, as a call is counted
    /// in once it has its slot: the waiter the slot goes to may take it on
    /// another thread at once, and the calls read as running never exceed the
    /// slots.
    fn release(&self) {
        self.calls.fetch_sub(RUNNING, Ordering::Relaxed);
        self.slots.add_permits(1);
    }
}

/// One half of [`Bulkhead::calls`] as a count.
fn half(count: u64) -> usize {
    usize::try_from(count).unwrap_or(usize::MAX)
}

/// One call counted among those waiting for a slot, until it is handed its
/// slot, or dropped: out of time, or its caller gone.
struct Waiting<'b>(&'b Bulkhead);

impl<'b> Waiting<'b> {
    fn among(bulkhead: &'b Bulkhead) -> Self {
        bulkhead.calls.fetch_add(WAITING, Ordering::Relaxed);
        Self(bulkhead)
    }

    /// The slot the call waited for, its own: the call leaves the ones
    /// waiting for the ones running in one step.
    fn handed(self) -> Slot<'b> {
        let bulkhead = self.0;
        std::mem::forget(self);
        bulkhead
            .calls
            .fetch_add(RUNNING - WAITING, Ordering::Relaxed);
        Slot(bulkhead)
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.calls.fetch_sub(WAITING, Ordering::Relaxed);
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
        let bulkhead = Bulkhead::new(1, None, None);
        let held = bulkhead.enter(None).await.expect("a free slot");
        assert_eq!(bulkhead.enter(None).await.err(), Some(Full::Slots));
        drop(held);
        assert!(bulkhead.enter(None).await.is_ok(), "the slot came back");
    }

    /// The wait for a slot never outlasts the unit the call runs in.
    #[tokio::test(start_paused = true)]
    async fn the_wait_for_a_slot_ends_at_the_units_deadline_when_that_is_sooner() {
        let bulkhead = Bulkhead::new(1, Some(Duration::from_secs(60)), None);
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
        let bulkhead = Bulkhead::new(1, Some(Duration::from_millis(500)), None);
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

    /// The reads count what a call arriving now meets: the slots taken, and
    /// the calls waiting for one, a waiter leaving the count however its wait
    /// ends — handed its slot, out of time, or dropped.
    #[tokio::test(start_paused = true)]
    async fn the_slots_taken_and_the_calls_waiting_are_read_as_they_stand() {
        let bulkhead = Bulkhead::new(2, Some(Duration::from_millis(500)), None);
        assert_eq!((bulkhead.running(), bulkhead.waiting()), (0, 0));
        let first = bulkhead.enter(None).await.expect("a free slot");
        let second = bulkhead.enter(None).await.expect("the other slot");
        assert_eq!((bulkhead.running(), bulkhead.waiting()), (2, 0));

        let mut handed = Box::pin(bulkhead.enter(None));
        let mut dropped = Box::pin(bulkhead.enter(None));
        assert!(waits(handed.as_mut()).await && waits(dropped.as_mut()).await);
        assert_eq!(
            (bulkhead.running(), bulkhead.waiting()),
            (2, 2),
            "no bound on the waiters, and both are counted"
        );
        drop(dropped);
        assert_eq!(bulkhead.waiting(), 1, "its caller gone");
        drop(first);
        let handed = handed.await.expect("the slot freed for it");
        assert_eq!((bulkhead.running(), bulkhead.waiting()), (2, 0));

        assert_eq!(bulkhead.enter(None).await.err(), Some(Full::Slots));
        assert_eq!(bulkhead.waiting(), 0, "its wait ran out");
        drop((second, handed));
        assert_eq!((bulkhead.running(), bulkhead.waiting()), (0, 0));
    }

    /// A slot freed while a call waits for it is that call's at once, and no
    /// other's, yet the call is read as waiting, and not as running, until
    /// its own future holds the slot: at the handover as at every instant it
    /// is counted once.
    #[tokio::test(start_paused = true)]
    async fn a_waiter_handed_a_freed_slot_is_read_as_waiting_until_it_holds_it() {
        let bulkhead = Bulkhead::new(1, Some(Duration::from_secs(60)), None);
        let first = bulkhead.enter(None).await.expect("a free slot");
        let mut handed = Box::pin(bulkhead.enter(None));
        assert!(waits(handed.as_mut()).await);
        assert_eq!((bulkhead.running(), bulkhead.waiting()), (1, 1));

        drop(first);
        assert_eq!(
            (bulkhead.running(), bulkhead.waiting()),
            (0, 1),
            "one call alive, yet to be polled with the slot freed for it"
        );
        assert!(
            bulkhead.try_enter().is_none(),
            "the slot is the waiter's, though it reads as waiting"
        );
        let slot = handed.await.expect("the slot freed for it");
        assert_eq!((bulkhead.running(), bulkhead.waiting()), (1, 0));
        drop(slot);
        assert_eq!((bulkhead.running(), bulkhead.waiting()), (0, 0));
    }

    /// A slot held past its run stays taken, and its call read as running,
    /// with no borrow of the bulkhead, until it is freed: once, for the one
    /// call it was taken for.
    #[cfg(feature = "reqwest")]
    #[tokio::test(start_paused = true)]
    async fn a_slot_held_past_its_run_stays_taken_and_read_as_running_until_it_is_freed() {
        let bulkhead = Bulkhead::new(2, None, None);
        let taken = bulkhead.try_enter().expect("a free slot");
        let held = bulkhead.hold(bulkhead.try_enter().expect("the other slot"));
        assert_eq!(bulkhead.enter(None).await.err(), Some(Full::Slots));
        assert_eq!(bulkhead.running(), 2, "the held slot's call still runs");

        bulkhead.free(held);
        assert_eq!(bulkhead.running(), 1, "freed, its call is counted out");
        let freed = bulkhead.try_enter().expect("the slot it held");
        assert!(
            bulkhead.try_enter().is_none(),
            "one slot came back, not two"
        );
        drop((taken, freed));
        assert_eq!(bulkhead.slots.available_permits(), 2);
        assert_eq!((bulkhead.running(), bulkhead.waiting()), (0, 0));
    }

    /// A call waiting for a slot, polled inside the wake that hands it one,
    /// as a waiter on another thread may be before the call freeing the slot
    /// has returned; what it answers is kept.
    struct PolledAtItsWake {
        entering: std::sync::Mutex<Option<Pin<Box<dyn Future<Output = usize> + Send>>>>,
        answered: std::sync::Mutex<Option<usize>>,
    }

    impl std::task::Wake for PolledAtItsWake {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            let mut entering = self.entering.lock().expect("one wake at a time");
            let Some(polled) = entering.as_mut() else {
                return;
            };
            let waker = std::task::Waker::from(Arc::clone(self));
            let polled = polled
                .as_mut()
                .poll(&mut std::task::Context::from_waker(&waker));
            if let Poll::Ready(answer) = polled {
                *entering = None;
                *self.answered.lock().expect("one wake at a time") = Some(answer);
            }
        }
    }

    /// The calls read as running never exceed the slots: the call freeing a
    /// slot is counted out before the waiter the slot goes to can take it,
    /// so a waiter taking it inside the wake reads itself alone.
    #[tokio::test(start_paused = true)]
    async fn a_freed_slot_is_counted_out_before_the_waiter_it_goes_to_takes_it() {
        let bulkhead = Arc::new(Bulkhead::new(1, Some(Duration::from_secs(60)), None));
        let held = bulkhead.enter(None).await.expect("a free slot");
        let waiter = Arc::new(PolledAtItsWake {
            entering: std::sync::Mutex::new(Some(Box::pin({
                let bulkhead = Arc::clone(&bulkhead);
                async move {
                    let _slot = bulkhead.enter(None).await.expect("the slot freed for it");
                    bulkhead.running()
                }
            }))),
            answered: std::sync::Mutex::new(None),
        });
        std::task::Wake::wake_by_ref(&waiter);
        assert_eq!((bulkhead.running(), bulkhead.waiting()), (1, 1));

        drop(held);
        let read = waiter.answered.lock().expect("one wake at a time").take();
        assert_eq!(
            read,
            Some(1),
            "handed the slot inside the wake, it read the calls running with its own"
        );
        assert_eq!((bulkhead.running(), bulkhead.waiting()), (0, 0));
    }

    /// A bound on the waiters means nothing where no call waits, as a wait
    /// means nothing without slots: with no wait it builds no place.
    #[test]
    fn a_bound_on_the_waiters_builds_its_places_only_beside_a_wait() {
        let wait = Some(Duration::from_secs(1));
        assert!(Bulkhead::new(1, None, Some(4)).waiters.is_none());
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
        let bulkhead = Bulkhead::new(1, Some(Duration::from_secs(60)), Some(1));
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
        let bulkhead = Bulkhead::new(1, Some(Duration::from_millis(500)), Some(1));
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
        let bulkhead = Bulkhead::new(1, Some(Duration::from_secs(60)), Some(1));
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
        let bulkhead = Arc::new(Bulkhead::new(
            1,
            Some(Duration::from_secs(60)),
            Some(WAITERS),
        ));
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
