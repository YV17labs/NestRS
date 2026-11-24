//! [`ResilienceBody`] — the body of a response [`ResilienceMiddleware`] hands
//! back: read as it comes, never buffered, holding what its run held of the
//! policy until it ends.
//!
//! [`ResilienceMiddleware`]: crate::ResilienceMiddleware

use std::convert::Infallible;
use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use http_body::{Body, Frame, SizeHint};
use nest_rs_core::{Instance, error_message};
use tokio::time::{Instant, Sleep};

use crate::error::{ResilienceError, TIMED_OUT};
use crate::policy::Held;

pin_project_lite::pin_project! {
    /// A response's body under the policy its request ran under: until it
    /// ends, it holds the run's slot of the bulkhead, the breaker's verdict on
    /// an answer whose head was judged a success, and what is left of the
    /// attempt's bound, which runs from the send.
    ///
    /// Read to its end, or dropped before it, the answer stands as its head
    /// was judged. A read that fails is a failure, and so is one still waiting
    /// for the dependency when the bound ends, which is given up there,
    /// answering a [`ResilienceError::TimedOut`]: a frame that came is handed
    /// on whenever it is read, so a reader slower than the dependency is no
    /// failure of the dependency's.
    pub(crate) struct ResilienceBody<I>
    where
        I: Instance,
    {
        #[pin]
        body: reqwest::Body,
        // Armed at the first read that waits.
        #[pin]
        bound: Option<Sleep>,
        // `None` once the answer is settled.
        held: Option<Held<I>>,
        // Whether the last read was left waiting for the dependency.
        waiting: bool,
    }

    impl<I> PinnedDrop for ResilienceBody<I>
    where
        I: Instance,
    {
        fn drop(this: Pin<&mut Self>) {
            let this = this.project();
            // A read cut at the instant the bound ends at, by a timer the reader
            // armed there, may be dropped before the body's own timer is polled.
            if *this.waiting
                && let Some(held) = this.held
                && Instant::now() >= held.ends()
            {
                held.settle(Some(TIMED_OUT));
            }
        }
    }
}

impl<I: Instance> ResilienceBody<I> {
    /// `body`, holding `held` until it ends.
    pub(crate) const fn new(body: reqwest::Body, held: Held<I>) -> Self {
        Self {
            body,
            bound: None,
            held: Some(held),
            waiting: false,
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
        let read = this.body.as_mut().poll_frame(cx);
        let Some(held) = this.held.as_mut() else {
            return read.map_err(Into::into);
        };
        match read {
            Poll::Ready(Some(Ok(frame))) => {
                *this.waiting = false;
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(None) => {
                *this.held = None;
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(error))) => {
                held.settle(Some(&error_message(&error)));
                *this.held = None;
                Poll::Ready(Some(Err(error.into())))
            }
            Poll::Pending => {
                if this.bound.is_none() {
                    this.bound.set(Some(tokio::time::sleep_until(held.ends())));
                }
                let bound = this.bound.as_mut().as_pin_mut();
                if bound.is_some_and(|bound| bound.poll(cx).is_pending()) {
                    *this.waiting = true;
                    return Poll::Pending;
                }
                let refusal = held.timed_out();
                *this.held = None;
                // The read is given up, and the connection it waited on with it.
                this.body.set(reqwest::Body::default());
                this.bound.set(None);
                Poll::Ready(Some(Err(
                    ResilienceError::<Infallible>::from(refusal).into()
                )))
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
    use std::sync::Arc;
    use std::time::Duration;

    use nest_rs_core::Unnamed;
    use nest_rs_testing::LogCapture;
    use tokio::sync::mpsc;

    use super::*;
    use crate::error::Refusal;
    use crate::policy::{ResiliencePolicy, Verdict};
    use crate::{BreakerState, ResilienceConfig, TARGET};

    const TIMEOUT: Duration = Duration::from_secs(5);
    const OPEN_FOR: Duration = Duration::from_secs(30);

    /// What a read of these tests fails on.
    const RESET: &str = "connection reset by peer";

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
        let failed = policy.run(|_| async { Err::<(), _>(io::Error::other(RESET)) });
        assert!(failed.await.is_err());
        tokio::time::advance(OPEN_FOR).await;
        policy
    }

    /// What feeds a body its frames, or the failure its read ends on; dropped,
    /// the body ends.
    type Feed = mpsc::UnboundedSender<Result<&'static [u8], io::Error>>;

    /// A body read as its test feeds it.
    struct Fed(mpsc::UnboundedReceiver<Result<&'static [u8], io::Error>>);

    impl Body for Fed {
        type Data = &'static [u8];
        type Error = io::Error;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            self.0
                .poll_recv(cx)
                .map(|fed| fed.map(|fed| fed.map(Frame::data)))
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
        let (feed, fed) = mpsc::unbounded_channel();
        let body = ResilienceBody::new(reqwest::Body::wrap(Fed(fed)), held);
        (Box::pin(body), feed)
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

    /// Whether a read of `body` waits for the dependency.
    async fn waits(body: &mut Covered) -> bool {
        std::future::poll_fn(|cx| Poll::Ready(body.as_mut().poll_frame(cx).is_pending())).await
    }

    /// Each transition `lines` holds, by the state it entered.
    fn states(lines: &LogCapture) -> Vec<String> {
        lines
            .find(TARGET, "circuit breaker changed state")
            .iter()
            .map(|line| line.field("state").unwrap_or_default())
            .collect()
    }

    /// What the breaker's one opening by a failure says it failed on.
    fn opened_on(lines: &LogCapture) -> Vec<String> {
        lines
            .find(TARGET, "circuit breaker changed state")
            .iter()
            .filter_map(|line| line.field("error"))
            .collect()
    }

    /// Whether `policy`'s one slot is taken, which a call arriving meets once
    /// its breaker lets it through.
    async fn slot_taken(policy: &ResiliencePolicy) -> bool {
        policy.begin(None).await.err() == Some(Refusal::Full)
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
        feed.send(Ok(b"half")).expect("the body is read");
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
                feed.send(Ok(b"half")).expect("the body is read");
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

        feed.send(Err(io::Error::other(RESET)))
            .expect("the body is read");
        let failed = read(&mut body)
            .await
            .and_then(Result::err)
            .expect("the read failed");
        assert!(
            nest_rs_core::error_message(failed.as_ref()).ends_with(RESET),
            "the failure as it came: {failed:?}"
        );
        assert!(
            nest_rs_core::__private::find_in_chain::<ResilienceError<Infallible>>(failed.as_ref())
                .is_none(),
            "no refusal of the policy's: {failed:?}"
        );
        assert!(matches!(
            policy.breaker_state(),
            Some(BreakerState::Open { .. })
        ));
        let opened = opened_on(&lines);
        assert_eq!(opened.len(), 1, "{opened:?}");
        assert!(opened[0].ends_with(RESET), "{opened:?}");
        policy.close_breaker().expect("a breaker");
        assert!(!slot_taken(&policy).await, "freed at the failure");
    }

    /// The bound runs from the send, the head and the body sharing it: a read
    /// still waiting when it ends is given up there, a failure counted as a
    /// timeout, its reader told the policy's own refusal with the bound it was
    /// given up at, and the slot is freed.
    #[tokio::test(start_paused = true)]
    async fn a_body_still_waiting_at_its_attempts_bound_is_given_up_as_timed_out() {
        let lines = LogCapture::install();
        let policy = policy(1);
        let sent = Instant::now();
        let head = Duration::from_secs(2);
        let (mut body, feed) = answered(&policy, head, Verdict::Succeeded).await;
        assert_eq!(sent.elapsed(), head);

        let given_up = read(&mut body)
            .await
            .and_then(Result::err)
            .expect("given up");
        assert_eq!(
            sent.elapsed(),
            TIMEOUT,
            "at the bound, counted from the send"
        );
        let refusal = given_up
            .downcast_ref::<ResilienceError<Infallible>>()
            .expect("the policy's refusal");
        assert!(
            matches!(refusal, ResilienceError::TimedOut { timeout, .. } if *timeout == TIMEOUT),
            "{refusal:?}"
        );
        assert_eq!(opened_on(&lines), [TIMED_OUT]);
        policy.close_breaker().expect("a breaker");
        assert!(!slot_taken(&policy).await, "freed as the read is given up");

        feed.send(Ok(b"late")).ok();
        assert!(read(&mut body).await.is_none(), "the body was let go");
    }

    /// A read its reader cuts at the instant the bound ends at — an edge's
    /// timer, due with the body's own and polled first — has timed out all the
    /// same; one dropped while it waits before that instant has not.
    #[tokio::test(start_paused = true)]
    async fn a_read_dropped_while_it_waits_has_timed_out_only_at_its_bounds_end() {
        for (waited, timed_out) in [(TIMEOUT - Duration::from_millis(1), false), (TIMEOUT, true)] {
            let lines = LogCapture::install();
            let policy = policy(1);
            let (mut body, _feed) = succeeded(&policy).await;
            assert!(waits(&mut body).await);
            tokio::time::advance(waited).await;
            drop(body);

            assert_eq!(
                matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
                timed_out,
                "dropped after {waited:?}"
            );
            assert_eq!(opened_on(&lines).len(), usize::from(timed_out));
            policy.close_breaker().expect("a breaker");
            assert!(!slot_taken(&policy).await, "dropped after {waited:?}");
        }
    }

    /// The bound is the dependency's, never its reader's: a frame that came
    /// is handed on however late it is read, and an answer held unread past
    /// its bound, then dropped, stands as its head was judged. Only a read
    /// left waiting past the bound is given up, at once.
    #[tokio::test(start_paused = true)]
    async fn a_frame_that_came_is_handed_on_past_the_bound_and_a_wait_past_it_is_given_up() {
        let policy = policy(1);
        let (unread, _feed) = succeeded(&policy).await;
        tokio::time::advance(2 * TIMEOUT).await;
        drop(unread);
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));

        let (mut body, feed) = succeeded(&policy).await;
        feed.send(Ok(b"in time")).expect("the body is read");
        tokio::time::advance(2 * TIMEOUT).await;
        assert_eq!(
            read(&mut body).await.and_then(Result::ok),
            Some(b"in time".to_vec())
        );
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));

        let late = Instant::now();
        assert!(matches!(read(&mut body).await, Some(Err(_))));
        assert_eq!(late.elapsed(), Duration::ZERO, "given up at once");
        assert!(matches!(
            policy.breaker_state(),
            Some(BreakerState::Open { .. })
        ));
    }

    /// A manual switch of the breaker while a body is read leaves that body's
    /// verdict nothing to change: its read failing opens nothing.
    #[tokio::test(start_paused = true)]
    async fn a_manual_switch_while_a_body_is_read_leaves_its_verdict_nothing_to_change() {
        let policy = policy(1);
        let (mut body, feed) = succeeded(&policy).await;
        policy.close_breaker().expect("a breaker");
        feed.send(Err(io::Error::other(RESET)))
            .expect("the body is read");
        assert!(matches!(read(&mut body).await, Some(Err(_))));
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
        assert!(!slot_taken(&policy).await, "the slot is freed all the same");
    }

    /// A head judged a failure counted when it came: its body holds the slot
    /// and the bound, and whatever its read ends on counts nothing more.
    #[tokio::test(start_paused = true)]
    async fn a_head_judged_a_failure_counts_once_whatever_its_body_ends_on() {
        let policy = policy(2);
        let failed = Verdict::Failed {
            asked: None,
            error: "the dependency answered 503".into(),
        };
        let (mut body, feed) = answered(&policy, Duration::ZERO, failed).await;
        assert!(slot_taken(&policy).await, "the unread body holds the slot");
        feed.send(Err(io::Error::other(RESET)))
            .expect("the body is read");
        assert!(matches!(read(&mut body).await, Some(Err(_))));
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Closed),
            "one failure of the two that open it"
        );
        assert!(!slot_taken(&policy).await);
    }
}
