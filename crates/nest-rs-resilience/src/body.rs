//! [`ResilienceBody`] — the body of a response [`ResilienceMiddleware`] hands
//! back: read as it comes, never buffered, holding what its run held of the
//! policy until it ends, and bounding how long its reader waits for the
//! dependency, never how long its reader takes.
//!
//! [`ResilienceMiddleware`]: crate::ResilienceMiddleware

use std::convert::Infallible;
use std::error::Error;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use http_body::{Body, Frame, SizeHint};
use nest_rs_core::Instance;
use parking_lot::Mutex;
use tokio::time::{Instant, Sleep};

use crate::breaker::Cut;
use crate::error::{ResilienceError, TIMED_OUT};
use crate::policy::Held;
use crate::told::told;

pin_project_lite::pin_project! {
    /// A response's body under the policy its request ran under: until it
    /// ends, it holds the run's slot of the bulkhead, the breaker's verdict on
    /// an answer whose head was judged a success, and what the head left of
    /// the attempt's bound, a budget its reads may wait in all.
    ///
    /// Only a wait for the dependency is taken from that budget: from the read
    /// that found nothing to the transport's signal that something came. A
    /// frame that came is handed on whenever it is read, so the time its reader
    /// takes between two reads, or away from a read it abandoned, is never the
    /// dependency's. A read still waiting once the budget is spent is given up
    /// there, answering a [`ResilienceError::TimedOut`]: a failure counted as a
    /// timeout. A read that fails is a failure told as its chain, and one that
    /// panics a failure counted before the unwind goes on, as a panic in an
    /// attempt's call is.
    ///
    /// Read to its end, the answer stands as its head was judged. Dropped
    /// before it, it is judged by what the dependency owes then: nothing while
    /// no read waits or the budget is not spent, and past it what one more
    /// read finds — a frame or the end stands as the head was judged, a
    /// failure counts as itself, and only a body still waiting has timed out.
    /// A read left waiting is past it too, whatever the budget has left, when
    /// what drops the body is the timeout of a policy around its reader
    /// ([`Cut`]), or when it waited across the deadline of the unit it is read
    /// in — begun before that instant, dropped at or past it, as the edge
    /// cutting that unit drops it: whatever its reader took of its own before
    /// the read moved the budget's end past that cut. That deadline is read
    /// where the body is dropped and arms no timer: a body handed on past the
    /// unit its request was sent in — a handler answering the response's
    /// stream, which its edge reads once the unit is over — is dropped under
    /// none, so a client leaving a read of it, the budget unspent, charges
    /// the dependency nothing.
    pub(crate) struct ResilienceBody<I>
    where
        I: Instance,
    {
        #[pin]
        body: reqwest::Body,
        // Armed while a read waits, due when the budget is spent.
        #[pin]
        cut: Option<Sleep>,
        // `None` once the answer is settled.
        held: Option<Held<I>>,
        // How long the reads may still wait for the dependency in all.
        left: Duration,
        // When the read still believed waiting found nothing.
        waiting: Option<Instant>,
        signal: Arc<Signal>,
        // `signal` as the waker the inner body is polled with, built once.
        waker: Waker,
    }

    impl<I> PinnedDrop for ResilienceBody<I>
    where
        I: Instance,
    {
        fn drop(this: Pin<&mut Self>) {
            let mut this = this.project();
            let (Some(held), Some(since)) = (this.held.as_mut(), *this.waiting) else {
                return;
            };
            let now = Instant::now();
            // The budget counts from the read, so its timer is due past the deadline an
            // edge cuts this reader at: read here, a body handed on past its unit under none.
            let across = || {
                nest_rs_core::__private::deadline()
                    .is_some_and(|deadline| since < deadline && deadline <= now)
            };
            let spent = now.saturating_duration_since(since) >= *this.left;
            if !(spent || across() || Cut::encloses()) {
                return;
            }
            // An edge cutting its unit as the budget is spent may drop the body
            // before its timer is polled, and an abandoned read is never polled again.
            let mut unheard = Context::from_waker(Waker::noop());
            let owed = catch_unwind(AssertUnwindSafe(|| {
                this.body.as_mut().poll_frame(&mut unheard)
            }));
            match owed {
                Ok(Poll::Pending) => {
                    held.settle(Some(TIMED_OUT));
                    // A body of another policy's this one reads is dropped by this timeout.
                    Cut::dropping(|| this.body.set(reqwest::Body::default()));
                }
                Ok(Poll::Ready(Some(Err(error)))) => failed(held, &error),
                Ok(Poll::Ready(Some(Ok(_)) | None)) => {}
                // A destructor does not unwind: the panic is counted here, and the
                // process hook has filed it.
                Err(_unwound) => held.panicked(),
            }
        }
    }
}

/// What the transport says of a read left waiting, whether or not its reader
/// is there to hear it: the inner body is polled with this as its waker, which
/// keeps the instant something came and wakes the reader of the latest read.
#[derive(Default)]
struct Signal {
    heard: Mutex<Heard>,
}

#[derive(Default)]
struct Heard {
    /// When the transport first signalled since the latest read.
    at: Option<Instant>,
    reader: Option<Waker>,
}

impl Signal {
    /// When the transport signalled since the read before this one, if it
    /// did, `reader` being the one it wakes from now on.
    fn listen(&self, reader: &Waker) -> Option<Instant> {
        // Cloned before the lock and dropped after it: either runs the
        // reader's runtime's code.
        let mut spare = Some(reader.clone());
        let at = {
            let mut heard = self.heard.lock();
            if !heard
                .reader
                .as_ref()
                .is_some_and(|known| known.will_wake(reader))
            {
                std::mem::swap(&mut heard.reader, &mut spare);
            }
            heard.at.take()
        };
        drop(spare);
        at
    }
}

impl Wake for Signal {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let reader = {
            let mut heard = self.heard.lock();
            heard.at.get_or_insert_with(Instant::now);
            heard.reader.take()
        };
        if let Some(reader) = reader {
            reader.wake();
        }
    }
}

/// A read of the answer's body failed on `error`: a failure, told as its chain.
fn failed<I>(held: &mut Held<I>, error: &reqwest::Error) {
    held.settle(Some(&told(error)));
}

impl<I: Instance> ResilienceBody<I> {
    /// `body`, holding `held` until it ends.
    pub(crate) fn new(body: reqwest::Body, held: Held<I>) -> Self {
        let signal = Arc::new(Signal::default());
        Self {
            body,
            cut: None,
            left: held.left(),
            held: Some(held),
            waiting: None,
            waker: Waker::from(Arc::clone(&signal)),
            signal,
        }
    }
}

impl<I: Instance> Body for ResilienceBody<I> {
    type Data = <reqwest::Body as Body>::Data;
    type Error = Box<dyn Error + Send + Sync>;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();
        let Some(held) = this.held.as_mut() else {
            return this.body.poll_frame(cx).map_err(Into::into);
        };
        // The reader's waker first: a signal between the two would wake no one.
        let signalled = this.signal.listen(cx.waker());
        let mut heard = Context::from_waker(this.waker);
        let read = match catch_unwind(AssertUnwindSafe(|| {
            this.body.as_mut().poll_frame(&mut heard)
        })) {
            Ok(read) => read,
            Err(unwound) => {
                held.panicked();
                *this.held = None;
                resume_unwind(unwound)
            }
        };
        let Poll::Ready(read) = read else {
            if this.waiting.is_none() {
                let now = Instant::now();
                *this.waiting = Some(now);
                this.cut
                    .set(Some(tokio::time::sleep_until(now + *this.left)));
            }
            let cut = this.cut.as_mut().as_pin_mut();
            if cut.is_some_and(|cut| cut.poll(cx).is_pending()) {
                return Poll::Pending;
            }
            let refusal = held.timed_out();
            *this.held = None;
            // Let go here, not when the response is dropped: its connection is the one
            // that stalled, and a body of another policy's it reads is cut by this timeout.
            Cut::dropping(|| this.body.set(reqwest::Body::default()));
            return Poll::Ready(Some(Err(
                ResilienceError::<Infallible>::from(refusal).into()
            )));
        };
        if let Some(since) = this.waiting.take() {
            let came = signalled.unwrap_or_else(Instant::now);
            *this.left = this
                .left
                .saturating_sub(came.saturating_duration_since(since));
            this.cut.set(None);
        }
        match read {
            Some(Ok(frame)) => Poll::Ready(Some(Ok(frame))),
            None => {
                *this.held = None;
                Poll::Ready(None)
            }
            Some(Err(error)) => {
                failed(held, &error);
                *this.held = None;
                Poll::Ready(Some(Err(error.into())))
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::ready;
    use std::time::Duration;

    use nest_rs_core::Unnamed;
    use nest_rs_testing::LogCapture;
    use tokio::sync::mpsc;

    use super::*;
    use crate::error::Refusal;
    use crate::policy::{PANICKED, ResiliencePolicy, Verdict};
    use crate::{BreakerState, ResilienceConfig, TARGET};

    const TIMEOUT: Duration = Duration::from_secs(5);
    const OPEN_FOR: Duration = Duration::from_secs(30);

    /// What a read of these tests fails on, an I/O error holding no other:
    /// said by `std`'s sentence for its kind, beneath reqwest's too.
    const RESET: io::ErrorKind = io::ErrorKind::ConnectionReset;

    /// One slot, and a breaker `failures` failures of its latest `failures`
    /// attempts open, which one probe closes.
    fn policy(failures: usize) -> Arc<ResiliencePolicy> {
        let config = ResilienceConfig {
            timeout: TIMEOUT,
            retries: None,
            breaker_window: Some(failures),
            breaker_failures: failures,
            breaker_open: OPEN_FOR,
            concurrency: Some(1),
            ..ResilienceConfig::default()
        };
        Arc::new(ResiliencePolicy::new(&config).expect("a valid policy"))
    }

    /// `policy`'s breaker opened by one failure, its open time passed: the
    /// next request is its probe.
    async fn probing() -> Arc<ResiliencePolicy> {
        let policy = policy(1);
        let failed = policy.run(|_| async { Err::<(), _>(io::Error::from(RESET)) });
        assert!(failed.await.is_err());
        tokio::time::advance(OPEN_FOR).await;
        policy
    }

    /// What the dependency does next, as a test scripts it.
    #[derive(Clone, Copy, Debug)]
    enum Part {
        Frame(&'static [u8]),
        /// The read fails on [`RESET`].
        Fails,
        /// The transport panics as it is read.
        Panics,
    }

    /// What feeds a body its script, each part as the test sends it; dropped,
    /// the body ends, and held with nothing sent, the dependency stalls.
    type Feed = mpsc::UnboundedSender<Part>;

    /// A body read as its test feeds it.
    struct Fed(mpsc::UnboundedReceiver<Part>);

    impl Body for Fed {
        type Data = &'static [u8];
        type Error = io::Error;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            Poll::Ready(match ready!(self.0.poll_recv(cx)) {
                Some(Part::Frame(data)) => Some(Ok(Frame::data(data))),
                Some(Part::Fails) => Some(Err(io::Error::from(RESET))),
                Some(Part::Panics) => panic!("the transport panicked"),
                None => None,
            })
        }
    }

    type Covered = Pin<Box<ResilienceBody<Unnamed>>>;

    /// The body of a response to a request run under `policy`, whose head
    /// came after `head` and was read as `judged`, and what feeds it.
    async fn answered(
        policy: &Arc<ResiliencePolicy>,
        head: Duration,
        judged: Verdict,
    ) -> (Covered, Feed) {
        let (feed, fed) = mpsc::unbounded_channel();
        let body = covering(policy, head, judged, reqwest::Body::wrap(Fed(fed))).await;
        (body, feed)
    }

    /// `body` as the body of a response to a request run under `policy`,
    /// whose head came after `head` and was read as `judged`.
    async fn covering(
        policy: &Arc<ResiliencePolicy>,
        head: Duration,
        judged: Verdict,
        body: reqwest::Body,
    ) -> Covered {
        let run = policy
            .begin(None)
            .await
            .expect("the request is let through");
        let call = |_| async move {
            tokio::time::sleep(head).await;
            Ok::<_, io::Error>(())
        };
        let (attempted, outstanding) = run.attempt_once(call, move |_| judged.clone()).await;
        let (head, _) = run.finish(attempted).expect("the request was sent");
        head.expect("the head answered");
        let held = run.hold(outstanding, Arc::clone(policy));
        Box::pin(ResilienceBody::new(body, held))
    }

    /// A body behind a head that came at once and was judged a success.
    async fn succeeded(policy: &Arc<ResiliencePolicy>) -> (Covered, Feed) {
        answered(policy, Duration::ZERO, Verdict::Succeeded).await
    }

    /// One read of `body`: a frame's bytes, the failure's whole chain, or
    /// `None` at its end.
    async fn read(body: &mut Covered) -> Option<Result<Vec<u8>, Box<dyn Error + Send + Sync>>> {
        let frame = std::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await?;
        Some(frame.map(|frame| {
            frame
                .into_data()
                .map(|data| data.to_vec())
                .unwrap_or_default()
        }))
    }

    /// Whether a read of `body` finds nothing, its reader not staying for
    /// what comes: a read abandoned as it is made.
    async fn waits(body: &mut Covered) -> bool {
        std::future::poll_fn(|cx| Poll::Ready(body.as_mut().poll_frame(cx).is_pending())).await
    }

    /// A read of `body` its reader stays in, which the dependency answers
    /// with a frame after `wait`.
    async fn waited(body: &mut Covered, feed: &Feed, wait: Duration) {
        let (read, ()) = tokio::join!(read(body), async {
            tokio::time::sleep(wait).await;
            feed.send(Part::Frame(b"part")).expect("the body is read");
        });
        assert!(
            matches!(read, Some(Ok(_))),
            "the frame it waited for: {read:?}"
        );
    }

    /// How long a read of `body` waits, the dependency sending nothing,
    /// before it is given up as timed out.
    async fn given_up_after(body: &mut Covered) -> Duration {
        let started = Instant::now();
        let given_up = read(body).await.and_then(Result::err).expect("given up");
        let refusal = given_up
            .downcast_ref::<ResilienceError<Infallible>>()
            .expect("the policy's refusal");
        assert!(
            matches!(refusal, ResilienceError::TimedOut { timeout, .. } if *timeout == TIMEOUT),
            "{refusal:?}"
        );
        started.elapsed()
    }

    /// Each transition `lines` holds, by the state it entered.
    fn states(lines: &LogCapture) -> Vec<String> {
        lines
            .find(TARGET, crate::breaker::MOVED)
            .iter()
            .map(|line| line.field("state").unwrap_or_default())
            .collect()
    }

    /// What each opening of the breaker by a failure says it failed on.
    fn opened_on(lines: &LogCapture) -> Vec<String> {
        lines
            .find(TARGET, crate::breaker::MOVED)
            .iter()
            .filter_map(|line| line.field("error"))
            .collect()
    }

    /// Whether `policy`'s one slot is taken, which a call arriving meets once
    /// its breaker lets it through.
    async fn slot_taken(policy: &ResiliencePolicy) -> bool {
        policy.begin(None).await.err() == Some(Refusal::Full)
    }

    fn is_open(policy: &ResiliencePolicy) -> bool {
        matches!(policy.breaker_state(), Some(BreakerState::Open { .. }))
    }

    /// The breaker's verdict and the slot wait for the body's end: a probe
    /// whose head answered keeps its turn while its body is read, closes the
    /// breaker at the end, once, and frees the slot there, the body not
    /// dropped yet.
    #[tokio::test(start_paused = true)]
    async fn a_body_read_to_its_end_stands_as_its_head_was_judged_and_frees_its_slot() {
        let lines = LogCapture::install();
        let policy = probing().await;
        let (mut body, feed) = succeeded(&policy).await;
        feed.send(Part::Frame(b"half")).expect("the body is read");
        assert_eq!(
            read(&mut body).await.and_then(Result::ok),
            Some(b"half".to_vec())
        );
        assert!(
            matches!(policy.begin(None).await.err(), Some(Refusal::Open(_))),
            "half read, the probe still holds its turn"
        );
        assert_eq!(policy.breaker_state(), Some(BreakerState::HalfOpen));

        drop(feed);
        assert!(read(&mut body).await.is_none(), "the body's end");
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
        assert!(
            !slot_taken(&policy).await,
            "freed at the end, before the drop"
        );
        drop(body);
        assert_eq!(states(&lines), ["open", "half_open", "closed"]);
    }

    /// A body its reader drops, unread or half read, stands as its head was
    /// judged: a client reading a status alone still closes a probe, and
    /// frees its slot at the drop.
    #[tokio::test(start_paused = true)]
    async fn a_body_dropped_before_its_end_stands_as_its_head_was_judged() {
        for half_read in [false, true] {
            let lines = LogCapture::install();
            let policy = probing().await;
            let (mut body, feed) = succeeded(&policy).await;
            if half_read {
                feed.send(Part::Frame(b"half")).expect("the body is read");
                assert!(read(&mut body).await.is_some());
                assert!(waits(&mut body).await, "the rest has not come");
            }
            assert_eq!(policy.breaker_state(), Some(BreakerState::HalfOpen));

            drop(body);
            assert_eq!(
                policy.breaker_state(),
                Some(BreakerState::Closed),
                "half read: {half_read}"
            );
            assert!(!slot_taken(&policy).await, "half read: {half_read}");
            assert_eq!(states(&lines), ["open", "half_open", "closed"]);
        }
    }

    /// A read that fails is the dependency failing: the breaker counts it,
    /// saying its chain, the slot is freed, and the failure is handed on as
    /// it came.
    #[tokio::test(start_paused = true)]
    async fn a_body_whose_read_fails_is_a_failure_saying_its_chain() {
        let lines = LogCapture::install();
        let policy = policy(1);
        let (mut body, feed) = succeeded(&policy).await;
        assert!(slot_taken(&policy).await, "the unread body holds the slot");
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));

        feed.send(Part::Fails).expect("the body is read");
        let failed = read(&mut body)
            .await
            .and_then(Result::err)
            .expect("the read failed");
        assert!(
            nest_rs_core::error_message(failed.as_ref()).ends_with(&RESET.to_string()),
            "the failure as it came: {failed:?}"
        );
        assert!(
            nest_rs_core::__private::find_in_chain::<ResilienceError<Infallible>>(failed.as_ref())
                .is_none(),
            "no refusal of the policy's: {failed:?}"
        );
        assert!(is_open(&policy));
        let opened = opened_on(&lines);
        assert_eq!(opened.len(), 1, "{opened:?}");
        assert!(opened[0].ends_with(&RESET.to_string()), "{opened:?}");
        policy.close_breaker().expect("a breaker");
        assert!(!slot_taken(&policy).await, "freed at the failure");
    }

    /// What the head left of the attempt's bound is what the body's reads may
    /// wait in all, whenever they are made: a read still waiting once it is
    /// spent is given up there, a failure counted as a timeout, its reader
    /// told the policy's own refusal with the attempt's bound, which files no
    /// line of its own, and the slot is freed.
    #[tokio::test(start_paused = true)]
    async fn a_read_still_waiting_once_the_bounds_rest_is_spent_is_given_up_as_timed_out() {
        let lines = LogCapture::install();
        let policy = policy(1);
        let sent = Instant::now();
        let head = Duration::from_secs(2);
        let (mut body, feed) = answered(&policy, head, Verdict::Succeeded).await;
        assert_eq!(sent.elapsed(), head);
        tokio::time::advance(2 * TIMEOUT).await;

        assert_eq!(
            given_up_after(&mut body).await,
            TIMEOUT - head,
            "what the head left, counted from the read"
        );
        assert_eq!(opened_on(&lines), [TIMED_OUT]);
        let filed = lines.events();
        assert_eq!(
            filed.iter().filter(|line| line.target == TARGET).count(),
            1,
            "the breaker's opening, and no line for the timeout its reader is told: {filed:?}"
        );
        policy.close_breaker().expect("a breaker");
        assert!(!slot_taken(&policy).await, "freed as the read is given up");

        feed.send(Part::Frame(b"late")).ok();
        assert!(read(&mut body).await.is_none(), "the body was let go");
    }

    /// The waits of every read are taken from one budget, and nothing else
    /// is: two reads each kept waiting two seconds, their reader away longer
    /// than the bound after each, leave the next one the fifth second.
    #[tokio::test(start_paused = true)]
    async fn the_waits_of_every_read_are_taken_from_one_budget() {
        let policy = policy(1);
        let (mut body, feed) = succeeded(&policy).await;
        for _ in 0..2 {
            waited(&mut body, &feed, Duration::from_secs(2)).await;
            tokio::time::advance(2 * TIMEOUT).await;
        }
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));

        assert_eq!(
            given_up_after(&mut body).await,
            TIMEOUT - Duration::from_secs(4)
        );
        assert!(is_open(&policy));
    }

    /// A reader slower than a prompt dependency is never cut: under a
    /// transport's flow control the dependency sends the next part only once
    /// its reader made room, so a read made long after the bound's instant
    /// waits a round trip, which is all it is charged.
    #[tokio::test(start_paused = true)]
    async fn a_reader_slower_than_a_prompt_dependency_is_never_cut() {
        const ROUND_TRIP: Duration = Duration::from_millis(1);
        const ROUNDS: u32 = 8;
        let lines = LogCapture::install();
        let policy = policy(1);
        let (mut body, feed) = succeeded(&policy).await;
        for _ in 0..ROUNDS {
            feed.send(Part::Frame(b"buffered"))
                .expect("the body is read");
            assert!(matches!(read(&mut body).await, Some(Ok(_))));
            waited(&mut body, &feed, ROUND_TRIP).await;
            tokio::time::advance(TIMEOUT).await;
        }
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
        assert!(opened_on(&lines).is_empty());

        assert_eq!(
            given_up_after(&mut body).await,
            TIMEOUT - ROUNDS * ROUND_TRIP,
            "charged its round trips alone"
        );
    }

    /// An answer held unread, and a frame read long after it came, cost the
    /// dependency nothing: the reader's own time is no wait.
    #[tokio::test(start_paused = true)]
    async fn an_answer_held_unread_and_a_frame_read_late_cost_the_dependency_nothing() {
        let policy = policy(1);
        let (unread, _feed) = succeeded(&policy).await;
        tokio::time::advance(2 * TIMEOUT).await;
        drop(unread);
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));

        let (mut body, feed) = succeeded(&policy).await;
        feed.send(Part::Frame(b"in time"))
            .expect("the body is read");
        tokio::time::advance(2 * TIMEOUT).await;
        assert_eq!(
            read(&mut body).await.and_then(Result::ok),
            Some(b"in time".to_vec())
        );
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
        assert_eq!(
            given_up_after(&mut body).await,
            TIMEOUT,
            "nothing was taken"
        );
    }

    /// A call refused beside a probe whose answer its caller holds is told
    /// that probe's bound, the policy's timeout — the longest the body's
    /// reads wait in all, whatever the head took of it — as the head comes
    /// and however long the answer is held: nothing gives a held probe up, so
    /// no refusal is told the tick of one being given up.
    #[tokio::test(start_paused = true)]
    async fn a_call_refused_beside_a_probes_held_answer_is_told_the_probes_bound() {
        let policy = probing().await;
        let (held, _feed) = answered(&policy, Duration::from_secs(1), Verdict::Succeeded).await;
        for held_for in [Duration::ZERO, TIMEOUT, 10 * TIMEOUT] {
            tokio::time::advance(held_for).await;
            let refused = policy.run(|_| async { Ok::<(), io::Error>(()) }).await;
            assert!(
                matches!(
                    refused,
                    Err(ResilienceError::Open { retry_after, .. }) if retry_after == TIMEOUT
                ),
                "held for {held_for:?} more: {refused:?}"
            );
        }
        drop(held);
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
    }

    /// A frame read ends its read's wait: the body dropped long after stands
    /// as its head was judged, however long that read had waited.
    #[tokio::test(start_paused = true)]
    async fn a_frame_read_after_a_wait_ends_the_wait() {
        let lines = LogCapture::install();
        let policy = policy(1);
        let (mut body, feed) = succeeded(&policy).await;
        waited(&mut body, &feed, Duration::from_secs(1)).await;
        tokio::time::advance(2 * TIMEOUT).await;
        drop(body);

        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
        assert!(opened_on(&lines).is_empty());
        drop(feed);
    }

    /// How many times a reader was woken.
    #[derive(Default)]
    struct Wakes(AtomicUsize);

    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A read left waiting is woken once, by the frame it waited for: the
    /// wait over, no timer of the body's is left to wake its reader.
    #[tokio::test(start_paused = true)]
    async fn a_wait_that_ended_leaves_no_timer_to_wake_its_reader() {
        let policy = policy(1);
        let (mut body, feed) = succeeded(&policy).await;
        let wakes = Arc::new(Wakes::default());
        let reader = Waker::from(Arc::clone(&wakes));
        let mut cx = Context::from_waker(&reader);

        assert!(body.as_mut().poll_frame(&mut cx).is_pending());
        feed.send(Part::Frame(b"part")).expect("the body is read");
        assert_eq!(wakes.0.load(Ordering::Relaxed), 1, "woken for the frame");
        assert!(matches!(
            body.as_mut().poll_frame(&mut cx),
            Poll::Ready(Some(Ok(_)))
        ));
        tokio::time::advance(2 * TIMEOUT).await;
        assert_eq!(wakes.0.load(Ordering::Relaxed), 1, "and by nothing since");
    }

    /// The first signal since a read is the one kept: a transport signalling
    /// twice before its reader comes back says when something first came.
    #[tokio::test(start_paused = true)]
    async fn the_first_signal_since_a_read_is_the_one_kept() {
        let signal = Arc::new(Signal::default());
        assert_eq!(signal.listen(Waker::noop()), None);
        let first = Instant::now();
        signal.wake_by_ref();
        tokio::time::advance(Duration::from_secs(1)).await;
        Arc::clone(&signal).wake();

        assert_eq!(signal.listen(Waker::noop()), Some(first));
        assert_eq!(signal.listen(Waker::noop()), None, "told once");
    }

    /// A read dropped while it waits has timed out once its wait reached what
    /// was left of the budget — at that instant, where an edge's timer due
    /// with the body's own may be polled first, and at any later one — and
    /// not before, whether the budget is whole or partly spent.
    #[tokio::test(start_paused = true)]
    async fn a_read_dropped_while_it_waits_has_timed_out_only_once_its_budget_is_spent() {
        const TICK: Duration = Duration::from_millis(1);
        for spent in [Duration::ZERO, Duration::from_secs(4)] {
            let left = TIMEOUT - spent;
            for (waited_for, timed_out) in [(left - TICK, false), (left, true), (3 * TIMEOUT, true)]
            {
                let lines = LogCapture::install();
                let policy = policy(1);
                let (mut body, feed) = succeeded(&policy).await;
                if !spent.is_zero() {
                    waited(&mut body, &feed, spent).await;
                    tokio::time::advance(2 * TIMEOUT).await;
                }
                assert!(waits(&mut body).await);
                tokio::time::advance(waited_for).await;
                drop(body);

                let said = format!("{spent:?} spent, dropped after {waited_for:?}");
                assert_eq!(is_open(&policy), timed_out, "{said}");
                let expected: &[&str] = if timed_out { &[TIMED_OUT] } else { &[] };
                assert_eq!(opened_on(&lines), expected, "{said}");
                policy.close_breaker().expect("a breaker");
                assert!(!slot_taken(&policy).await, "{said}");
                drop(feed);
            }
        }
    }

    /// A read its reader abandoned, the response dropped long after: the
    /// answer is judged by what the dependency owes at the drop, never by
    /// the read left behind. A frame or the body's end that came since, in
    /// time or late, stands as the head was judged, and a failure that came
    /// counts as itself.
    #[tokio::test(start_paused = true)]
    async fn a_read_abandoned_then_dropped_late_is_judged_by_what_the_dependency_owes_then() {
        let in_time = Duration::from_secs(1);
        for (came, after, failed) in [
            (Some(Part::Frame(b"whole")), in_time, false),
            (Some(Part::Frame(b"whole")), 2 * TIMEOUT, false),
            (None, in_time, false),
            (Some(Part::Fails), in_time, true),
        ] {
            let lines = LogCapture::install();
            let policy = policy(1);
            let (mut body, feed) = succeeded(&policy).await;
            assert!(waits(&mut body).await, "a read its reader abandons");
            tokio::time::advance(after).await;
            let feed = came.map(|part| {
                feed.send(part).expect("the body is held");
                feed
            });
            tokio::time::advance(3 * TIMEOUT).await;
            drop(body);

            let said = format!("{came:?} after {after:?}");
            assert_eq!(is_open(&policy), failed, "{said}");
            let opened = opened_on(&lines);
            assert_eq!(opened.len(), usize::from(failed), "{said}: {opened:?}");
            let reset = RESET.to_string();
            assert!(opened.iter().all(|error| error.ends_with(&reset)), "{said}");
            policy.close_breaker().expect("a breaker");
            assert!(!slot_taken(&policy).await, "{said}");
            drop(feed);
        }
    }

    /// A read abandoned and taken up again later is charged what the
    /// dependency took, never the time its reader was away: the frame that
    /// came a second into the read, read three seconds after that, took one
    /// second of the budget.
    #[tokio::test(start_paused = true)]
    async fn a_read_abandoned_and_taken_up_again_is_charged_what_the_dependency_took() {
        let took = Duration::from_secs(1);
        let policy = policy(1);
        let (mut body, feed) = succeeded(&policy).await;
        assert!(waits(&mut body).await, "a read its reader abandons");
        tokio::time::advance(took).await;
        feed.send(Part::Frame(b"part")).expect("the body is read");
        tokio::time::advance(Duration::from_secs(3)).await;
        assert!(matches!(read(&mut body).await, Some(Ok(_))));
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));

        assert_eq!(given_up_after(&mut body).await, TIMEOUT - took);
    }

    /// A read abandoned and taken up again past the budget finds what the
    /// dependency owes then: a frame that came, however late, is handed on,
    /// and a body still waiting is given up at once.
    #[tokio::test(start_paused = true)]
    async fn a_read_taken_up_again_past_the_budget_is_handed_what_came_or_given_up_at_once() {
        for came in [false, true] {
            let policy = policy(1);
            let (mut body, feed) = succeeded(&policy).await;
            assert!(waits(&mut body).await, "a read its reader abandons");
            tokio::time::advance(2 * TIMEOUT).await;
            if came {
                feed.send(Part::Frame(b"late")).expect("the body is read");
                tokio::time::advance(TIMEOUT).await;
                assert_eq!(
                    read(&mut body).await.and_then(Result::ok),
                    Some(b"late".to_vec())
                );
                assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
            }

            assert_eq!(
                given_up_after(&mut body).await,
                Duration::ZERO,
                "came: {came}"
            );
            assert!(is_open(&policy), "came: {came}");
        }
    }

    /// A panic while the body is read is a failure, as a panic in an
    /// attempt's call is: a probe's opens the breaker again, once, and the
    /// slot is freed. At a read it is counted and unwinds on; at the read a
    /// drop past the budget makes it is counted and goes no further, a
    /// destructor never unwinding.
    #[tokio::test(start_paused = true)]
    async fn a_body_whose_read_panics_is_a_failure_counted_before_the_unwind_goes_on() {
        for at_its_drop in [false, true] {
            let lines = LogCapture::install();
            let policy = probing().await;
            let (mut body, feed) = succeeded(&policy).await;
            feed.send(Part::Frame(b"half")).expect("the body is read");
            assert!(matches!(read(&mut body).await, Some(Ok(_))));

            if at_its_drop {
                assert!(waits(&mut body).await);
                tokio::time::advance(TIMEOUT).await;
                feed.send(Part::Panics).expect("the body is held");
            } else {
                feed.send(Part::Panics).expect("the body is read");
                let unwound = nest_rs_core::panic::contain(read(&mut body)).await;
                assert!(unwound.is_err(), "the unwind goes on");
            }
            drop(body);

            assert_eq!(
                states(&lines),
                ["open", "half_open", "open"],
                "at its drop: {at_its_drop}"
            );
            assert_eq!(
                opened_on(&lines).last().map(String::as_str),
                Some(PANICKED),
                "at its drop: {at_its_drop}"
            );
            policy.close_breaker().expect("a breaker");
            assert!(!slot_taken(&policy).await, "at its drop: {at_its_drop}");
        }
    }

    /// A panic of the reader's own code while it holds a response is no
    /// failure of the dependency's: the answer stands as its head was judged.
    #[tokio::test(start_paused = true)]
    async fn a_panic_of_the_readers_own_code_charges_the_dependency_nothing() {
        let lines = LogCapture::install();
        let policy = probing().await;
        let (mut body, feed) = succeeded(&policy).await;
        feed.send(Part::Frame(b"half")).expect("the body is read");

        let reading = async move {
            let read = read(&mut body).await;
            assert!(read.is_none(), "the reader's own code panics on {read:?}");
        };
        assert!(nest_rs_core::panic::contain(reading).await.is_err());
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
        assert_eq!(states(&lines), ["open", "half_open", "closed"]);
        assert!(!slot_taken(&policy).await);
    }

    /// A manual switch of the breaker while a body is read leaves that body's
    /// verdict nothing to change: its read failing opens nothing.
    #[tokio::test(start_paused = true)]
    async fn a_manual_switch_while_a_body_is_read_leaves_its_verdict_nothing_to_change() {
        let policy = policy(1);
        let (mut body, feed) = succeeded(&policy).await;
        policy.close_breaker().expect("a breaker");
        feed.send(Part::Fails).expect("the body is read");
        assert!(matches!(read(&mut body).await, Some(Err(_))));
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
        assert!(!slot_taken(&policy).await, "the slot is freed all the same");
    }

    /// A head judged a failure counted when it came: its body holds the slot
    /// and the budget, and whatever its read ends on counts nothing more.
    #[tokio::test(start_paused = true)]
    async fn a_head_judged_a_failure_counts_once_whatever_its_body_ends_on() {
        let policy = policy(2);
        let failed = Verdict::Failed {
            asked: None,
            error: "the dependency answered 503".into(),
        };
        let (mut body, feed) = answered(&policy, Duration::ZERO, failed).await;
        assert!(slot_taken(&policy).await, "the unread body holds the slot");
        feed.send(Part::Fails).expect("the body is read");
        assert!(matches!(read(&mut body).await, Some(Err(_))));
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Closed),
            "one failure of the two that open it"
        );
        assert!(!slot_taken(&policy).await);
    }

    /// Half the timeout of [`policy`]: what the policy around a reader gives
    /// its attempt, and what a body over another's has to wait in.
    const SHORT: Duration = Duration::from_secs(2);

    /// A policy like [`policy`]'s, each attempt given up past [`SHORT`].
    fn short() -> Arc<ResiliencePolicy> {
        let config = ResilienceConfig {
            timeout: SHORT,
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: OPEN_FOR,
            ..ResilienceConfig::default()
        };
        Arc::new(ResiliencePolicy::new(&config).expect("a valid policy"))
    }

    /// What `policy`'s window counts: its outcomes, and the failures among them.
    fn counted(policy: &ResiliencePolicy) -> (usize, usize) {
        let stats = policy.breaker_stats().expect("a breaker");
        (stats.outcomes, stats.failures)
    }

    /// A body whose read waits as the timeout of the policy its reader runs
    /// under drops it has timed out, whatever its own budget has left: a
    /// dependency that sends a head and stalls opens its own breaker, however
    /// short the timeout around the call. Held with no read waiting, it
    /// stands as its head was judged.
    #[tokio::test(start_paused = true)]
    async fn a_body_whose_read_waits_as_the_timeout_of_a_policy_around_drops_it_has_timed_out() {
        let lines = LogCapture::install();
        let around = short();

        let stalled = policy(1);
        let (mut body, feed) = succeeded(&stalled).await;
        let cut = around
            .run(move |_| async move {
                let read = read(&mut body).await;
                Err::<(), _>(io::Error::other(format!("the body answered {read:?}")))
            })
            .await;
        assert!(
            matches!(cut, Err(ResilienceError::TimedOut { timeout, .. }) if timeout == SHORT),
            "{cut:?}"
        );
        assert!(is_open(&stalled), "{:?}", stalled.breaker_state());
        assert!(!slot_taken(&stalled).await);
        drop(feed);

        let unread = policy(1);
        let (body, _feed) = succeeded(&unread).await;
        around.close_breaker().expect("a breaker");
        let cut = around
            .run(move |_| async move {
                let _held = body;
                std::future::pending::<Result<(), io::Error>>().await
            })
            .await;
        assert!(
            matches!(cut, Err(ResilienceError::TimedOut { .. })),
            "{cut:?}"
        );
        assert_eq!(counted(&unread), (1, 0), "its head's success");
        assert_eq!(
            opened_on(&lines),
            [TIMED_OUT, TIMED_OUT, TIMED_OUT],
            "the stalled body's policy, and the one around each call"
        );
    }

    /// A body read through another policy's — two middlewares on one client —
    /// is dropped by that one's cut: its read still waiting, it has timed out
    /// with it, at the cut read and as a body past its budget is dropped;
    /// dropped with the one over it inside both budgets, each stands as its
    /// head was judged.
    #[tokio::test(start_paused = true)]
    async fn a_body_read_through_another_policys_times_out_with_the_cut_that_drops_it() {
        /// A body under [`policy`], read through one under [`short`].
        async fn stacked() -> (Arc<ResiliencePolicy>, Arc<ResiliencePolicy>, Covered, Feed) {
            let (inner, outer) = (policy(1), short());
            let (body, feed) = succeeded(&inner).await;
            let over = covering(
                &outer,
                Duration::ZERO,
                Verdict::Succeeded,
                reqwest::Body::wrap(body),
            )
            .await;
            (inner, outer, over, feed)
        }

        let (inner, outer, mut body, _feed) = stacked().await;
        let started = Instant::now();
        assert!(matches!(read(&mut body).await, Some(Err(_))), "cut");
        assert_eq!(started.elapsed(), SHORT);
        assert!(is_open(&outer) && is_open(&inner), "both timed out");
        assert!(!slot_taken(&inner).await);

        let (inner, outer, mut body, _feed) = stacked().await;
        assert!(waits(&mut body).await);
        tokio::time::advance(SHORT).await;
        drop(body);
        assert!(is_open(&outer) && is_open(&inner), "both timed out");

        let (inner, outer, mut body, _feed) = stacked().await;
        assert!(waits(&mut body).await);
        tokio::time::advance(SHORT / 2).await;
        drop(body);
        assert_eq!(
            (counted(&outer), counted(&inner)),
            ((1, 0), (1, 0)),
            "dropped inside both budgets"
        );
    }

    /// How far the deadline of a unit of these tests is: sooner than [`TIMEOUT`].
    const DEADLINE: Duration = Duration::from_secs(1);

    /// What `unit` answered under a deadline [`DEADLINE`] away, or `None`
    /// once its edge cut it there, the edge's own timer polled first.
    async fn cut_at_its_deadline<F: Future>(unit: F) -> Option<F::Output> {
        let deadline = Instant::now() + DEADLINE;
        let unit = nest_rs_core::__private::with_deadline(Some(deadline), unit);
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(deadline) => None,
            answered = unit => Some(answered),
        }
    }

    /// The body of a request sent as the unit polling this begins, its head
    /// answered at once and judged a success, reading `fed`.
    async fn sent_in_its_unit(
        policy: &Arc<ResiliencePolicy>,
        fed: mpsc::UnboundedReceiver<Part>,
    ) -> Covered {
        let fed = reqwest::Body::wrap(Fed(fed));
        covering(policy, Duration::ZERO, Verdict::Succeeded, fed).await
    }

    /// A read waiting as the edge cuts its unit at the deadline has timed out,
    /// whatever its reader took of its own between the head and the read: the
    /// budget counts from the read, so that time moves the body's own timer
    /// past the deadline, where the edge always cuts first.
    #[tokio::test(start_paused = true)]
    async fn a_read_waiting_across_its_units_deadline_has_timed_out_whatever_its_reader_took() {
        for own in [0, 2, 5, 400].map(Duration::from_millis) {
            let lines = LogCapture::install();
            let policy = policy(1);
            let (feed, fed) = mpsc::unbounded_channel();
            let cut = cut_at_its_deadline(async {
                let mut body = sent_in_its_unit(&policy, fed).await;
                tokio::time::sleep(own).await;
                read(&mut body).await
            })
            .await;

            assert!(cut.is_none(), "the reader's own {own:?}: {cut:?}");
            assert!(is_open(&policy), "the reader's own {own:?}");
            assert_eq!(opened_on(&lines), [TIMED_OUT], "the reader's own {own:?}");
            policy.close_breaker().expect("a breaker");
            assert!(!slot_taken(&policy).await, "the reader's own {own:?}");
            drop(feed);
        }
    }

    /// A body its unit's edge drops at the deadline is judged by what the
    /// dependency owes there: nothing while no read waits, its reader busy
    /// with work of its own; and with a read left waiting across the
    /// deadline, a frame or the end that came since stands as the head was
    /// judged, a failure counts as itself, and a body still waiting has timed
    /// out.
    #[tokio::test(start_paused = true)]
    async fn a_body_dropped_at_its_units_deadline_is_judged_by_what_the_dependency_owes_there() {
        let reset = RESET.to_string();
        for (reads, came, failed_on) in [
            (false, None, None),
            (true, Some(Some(Part::Frame(b"whole"))), None),
            (true, Some(None), None),
            (true, Some(Some(Part::Fails)), Some(reset.as_str())),
            (true, None, Some(TIMED_OUT)),
        ] {
            let lines = LogCapture::install();
            let policy = policy(1);
            let (feed, fed) = mpsc::unbounded_channel();
            let mut feed = Some(feed);
            let cut = cut_at_its_deadline(async {
                let mut body = sent_in_its_unit(&policy, fed).await;
                tokio::time::sleep(Duration::from_millis(2)).await;
                if reads {
                    assert!(waits(&mut body).await, "a read its reader abandons");
                }
                match (came, feed.as_ref()) {
                    (Some(Some(part)), Some(feed)) => feed.send(part).expect("the body is held"),
                    (Some(None), _) => drop(feed.take()),
                    _ => {}
                }
                std::future::pending::<()>().await;
                drop(body);
            })
            .await;

            let said = format!("a read waiting: {reads}, {came:?} came");
            assert!(cut.is_none(), "{said}");
            let opened = opened_on(&lines);
            assert_eq!(opened.len(), usize::from(failed_on.is_some()), "{said}");
            assert!(
                opened
                    .iter()
                    .zip(failed_on)
                    .all(|(on, failed_on)| on.ends_with(failed_on)),
                "{said}: {opened:?}"
            );
            assert_eq!(is_open(&policy), failed_on.is_some(), "{said}");
            policy.close_breaker().expect("a breaker");
            assert!(!slot_taken(&policy).await, "{said}");
        }
    }

    /// A read its reader drops before its unit's deadline, the budget
    /// unspent, charges the dependency nothing: only one left waiting across
    /// that instant is past its bound.
    #[tokio::test(start_paused = true)]
    async fn a_read_dropped_before_its_units_deadline_stands_as_its_head_was_judged() {
        let policy = policy(1);
        let (_feed, fed) = mpsc::unbounded_channel();
        let dropped = cut_at_its_deadline(async {
            let mut body = sent_in_its_unit(&policy, fed).await;
            assert!(waits(&mut body).await, "a read its reader abandons");
            tokio::time::sleep(DEADLINE / 2).await;
            drop(body);
        })
        .await;

        assert!(dropped.is_some(), "the unit ended before its deadline");
        assert_eq!(counted(&policy), (1, 0), "its head's success");
    }

    /// A body handed on past its unit — a handler answering the response's
    /// stream, which its edge reads once the unit is over — is bounded by its
    /// budget alone: a read waiting as that unit's deadline passes, and one
    /// begun after it, are handed what comes, and a client leaving a read
    /// begun after it charges the dependency nothing.
    #[tokio::test(start_paused = true)]
    async fn a_body_handed_on_past_its_unit_is_read_past_its_deadline_and_stands_as_its_client_leaves()
     {
        const PART: Duration = Duration::from_millis(300);
        let lines = LogCapture::install();
        let policy = policy(1);
        let (feed, fed) = mpsc::unbounded_channel();
        let sent = Instant::now();
        let mut body = cut_at_its_deadline(sent_in_its_unit(&policy, fed))
            .await
            .expect("the unit answers its stream before its deadline");

        tokio::time::advance(DEADLINE - PART / 3).await;
        waited(&mut body, &feed, PART).await;
        assert!(
            sent.elapsed() > DEADLINE,
            "its wait began before the deadline"
        );
        waited(&mut body, &feed, PART).await;
        assert!(waits(&mut body).await, "a read begun past the deadline");
        tokio::time::advance(PART).await;
        drop(body);

        assert_eq!(counted(&policy), (1, 0), "its head's success");
        assert!(opened_on(&lines).is_empty());
        assert!(!slot_taken(&policy).await);
    }

    /// A read begun at or past its unit's deadline, its reader busy until then
    /// and the edge's cut late, gave the dependency no time before that
    /// instant: dropped with its unit, the budget unspent, it charges nothing.
    #[tokio::test(start_paused = true)]
    async fn a_read_begun_at_or_past_its_units_deadline_charges_nothing_as_its_unit_is_dropped() {
        for past in [0, 100].map(Duration::from_millis) {
            let policy = policy(1);
            let (_feed, fed) = mpsc::unbounded_channel();
            let deadline = Instant::now() + DEADLINE;
            let unit = nest_rs_core::__private::with_deadline(Some(deadline), async {
                let mut body = sent_in_its_unit(&policy, fed).await;
                tokio::time::sleep(DEADLINE + past).await;
                assert!(waits(&mut body).await, "a read its reader abandons");
                std::future::pending::<()>().await;
                drop(body);
            });
            let cut = tokio::time::timeout(DEADLINE + past * 2, unit).await;

            assert!(cut.is_err(), "{past:?} past: {cut:?}");
            assert_eq!(
                counted(&policy),
                (1, 0),
                "{past:?} past: its head's success"
            );
            assert!(!slot_taken(&policy).await, "{past:?} past");
        }
    }

    /// A client leaving while the edge reads a body handed on past its unit
    /// charges the dependency nothing, the budget unspent — whenever it
    /// leaves: the one read that straddles the deadline of the unit the
    /// request was sent in is dropped under no deadline, as the reads before
    /// and after it are.
    #[tokio::test(start_paused = true)]
    async fn a_client_leaving_a_read_that_straddles_the_deadline_of_a_unit_over_charges_nothing() {
        const EITHER_SIDE: Duration = Duration::from_millis(100);
        let lines = LogCapture::install();
        let policy = policy(1);
        let (_feed, fed) = mpsc::unbounded_channel();
        let mut body = cut_at_its_deadline(sent_in_its_unit(&policy, fed))
            .await
            .expect("the unit answers its stream before its deadline");

        tokio::time::advance(DEADLINE - EITHER_SIDE).await;
        assert!(waits(&mut body).await, "a read begun before the deadline");
        tokio::time::advance(EITHER_SIDE * 2).await;
        assert!(
            nest_rs_core::__private::deadline().is_none(),
            "the edge reads the stream outside the unit"
        );
        drop(body);

        assert_eq!(counted(&policy), (1, 0), "its head's success");
        assert!(opened_on(&lines).is_empty());
        assert!(!slot_taken(&policy).await);
    }

    /// A `tracing` subscriber panicking out of each line a breaker's
    /// transition files.
    struct Failing;

    /// Whether an event is a breaker's transition.
    #[derive(Default)]
    struct Moved(bool);

    impl tracing::field::Visit for Moved {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0 |= field.name() == "message" && format!("{value:?}") == crate::breaker::MOVED;
        }
    }

    impl tracing::Subscriber for Failing {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            let mut moved = Moved::default();
            event.record(&mut moved);
            assert!(!moved.0, "the subscriber fails on a transition's line");
        }

        fn enter(&self, _: &tracing::span::Id) {}

        fn exit(&self, _: &tracing::span::Id) {}
    }

    /// A line panicking out of the transition a body's end makes — a probe's
    /// closing the breaker — unwinds out of its drop with the slot freed: the
    /// breaker moved, and no call is refused for a slot nothing holds.
    #[tokio::test(start_paused = true)]
    async fn a_line_panicking_as_a_body_is_dropped_leaves_its_slot_free() {
        let policy = probing().await;
        let (body, _feed) = succeeded(&policy).await;
        let lines = tracing::subscriber::set_default(Failing);
        let dropped = catch_unwind(AssertUnwindSafe(|| drop(body)));
        drop(lines);
        assert!(dropped.is_err(), "the closing's line");
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
        assert!(!slot_taken(&policy).await);
    }
}
