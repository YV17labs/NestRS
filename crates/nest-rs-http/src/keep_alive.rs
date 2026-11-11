//! [`KeepAlive`], the keepalive every edge holding a peer's channel open runs: a
//! ping once the peer has been silent for the interval — RFC 6455 §5.5.2's Ping
//! on a gateway's and a graphql-ws socket, MCP's `ping` request on a session's
//! stream — and the peer given up when nothing comes back within the timeout of
//! it.

use std::future::Future as _;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::time::{Instant, Sleep};

/// What an edge owes its peer once [`KeepAlive::poll_due`] is ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "a keepalive has these two outcomes, and each edge holding one matches both"
)]
pub enum Due {
    /// The peer has been silent for the interval: send it a ping.
    Ping,
    /// Nothing came back within the timeout of the ping: the peer is gone, and
    /// what it held is dropped.
    Silent,
}

/// A peer's keepalive: one timer, armed at the instant the next ping or the
/// end of its wait is due, so a message from the peer costs a clock read, and
/// the timer moves when it fires or when a ping is answered.
pub struct KeepAlive(Option<Pinging>);

struct Pinging {
    interval: Duration,
    timeout: Duration,
    timer: Pin<Box<Sleep>>,
    /// When the peer's last message arrived, or the keepalive started.
    heard_at: Instant,
    /// A ping is out and nothing has come back since.
    awaiting: bool,
}

impl KeepAlive {
    /// A keepalive sending a ping once the peer is silent for `interval`, and
    /// giving the peer up when nothing comes back within `timeout`; `None`
    /// sends none and never gives up.
    pub fn new(interval: Option<Duration>, timeout: Duration) -> Self {
        Self(interval.map(|interval| {
            let now = Instant::now();
            Pinging {
                interval,
                timeout,
                timer: Box::pin(tokio::time::sleep_until(now + interval)),
                heard_at: now,
                awaiting: false,
            }
        }))
    }

    /// A message of any kind came from the peer: it is there. The first one
    /// after a ping moves the timer to the next ping's instant, once per ping,
    /// so a wait past the interval never holds the next one back.
    pub fn heard(&mut self) {
        self.heard_at(Instant::now());
    }

    /// [`heard`](Self::heard), for a message that arrived `at`, read later by
    /// an edge whose peer is heard on another task than the one pinging it.
    pub fn heard_at(&mut self, at: Instant) {
        if let Some(pinging) = &mut self.0 {
            pinging.heard_at = at;
            if pinging.awaiting {
                pinging.awaiting = false;
                pinging.timer.as_mut().reset(at + pinging.interval);
            }
        }
    }

    /// Ready once a ping is due or the peer let one go unanswered past the
    /// timeout; never when the keepalive is off. Ready again with
    /// [`Due::Silent`] once given up.
    pub fn poll_due(&mut self, cx: &mut Context<'_>) -> Poll<Due> {
        let Some(pinging) = &mut self.0 else {
            return Poll::Pending;
        };
        loop {
            if pinging.timer.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
            if pinging.awaiting {
                return Poll::Ready(Due::Silent);
            }
            let now = Instant::now();
            let next = pinging.heard_at + pinging.interval;
            if next > now {
                pinging.timer.as_mut().reset(next);
                continue;
            }
            pinging.awaiting = true;
            pinging.timer.as_mut().reset(now + pinging.timeout);
            return Poll::Ready(Due::Ping);
        }
    }

    /// [`poll_due`](Self::poll_due) as a future, cancel-safe: dropped unfinished,
    /// it loses nothing.
    pub async fn due(&mut self) -> Due {
        std::future::poll_fn(|cx| self.poll_due(cx)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_secs(20);
    const TIMEOUT: Duration = Duration::from_secs(5);

    /// What the keepalive owes at the instant it is ready, counted from its start.
    async fn next_due(keep_alive: &mut KeepAlive, start: Instant) -> (Due, Duration) {
        let due = keep_alive.due().await;
        (due, Instant::now() - start)
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_peer_is_pinged_at_the_interval_then_given_up_at_the_timeout() {
        let start = Instant::now();
        let mut keep_alive = KeepAlive::new(Some(INTERVAL), TIMEOUT);

        assert_eq!(
            next_due(&mut keep_alive, start).await,
            (Due::Ping, INTERVAL)
        );
        assert_eq!(
            next_due(&mut keep_alive, start).await,
            (Due::Silent, INTERVAL + TIMEOUT)
        );
        assert_eq!(
            next_due(&mut keep_alive, start).await,
            (Due::Silent, INTERVAL + TIMEOUT),
            "given up, it stays given up",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_to_the_ping_defers_the_next_one_by_the_interval() {
        let start = Instant::now();
        let mut keep_alive = KeepAlive::new(Some(INTERVAL), TIMEOUT);
        assert_eq!(next_due(&mut keep_alive, start).await.0, Due::Ping);

        tokio::time::sleep(Duration::from_secs(1)).await;
        keep_alive.heard();

        assert_eq!(
            next_due(&mut keep_alive, start).await,
            (Due::Ping, INTERVAL + Duration::from_secs(1) + INTERVAL),
            "never given up, pinged an interval after the answer",
        );
    }

    /// However long the Ping waits, an answer moves the next one to an interval
    /// after it: a timeout past the interval never leaves the peer silent for
    /// longer than the interval before it is pinged.
    #[tokio::test(start_paused = true)]
    async fn an_answer_to_the_ping_defers_the_next_one_by_the_interval_alone() {
        const LONG_WAIT: Duration = Duration::from_secs(60);
        let start = Instant::now();
        let mut keep_alive = KeepAlive::new(Some(INTERVAL), LONG_WAIT);
        assert_eq!(
            next_due(&mut keep_alive, start).await,
            (Due::Ping, INTERVAL)
        );

        tokio::time::sleep(Duration::from_secs(1)).await;
        keep_alive.heard();

        assert_eq!(
            next_due(&mut keep_alive, start).await,
            (Due::Ping, INTERVAL + Duration::from_secs(1) + INTERVAL),
            "pinged an interval after the answer, not when the wait would have ended",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_peer_that_keeps_talking_is_never_pinged() {
        let start = Instant::now();
        let mut keep_alive = KeepAlive::new(Some(INTERVAL), TIMEOUT);
        for _ in 0..10 {
            tokio::select! {
                biased;
                due = keep_alive.due() => panic!("{due:?} owed to a talking peer"),
                () = tokio::time::sleep(INTERVAL - Duration::from_secs(1)) => keep_alive.heard(),
            }
        }
        assert_eq!(
            next_due(&mut keep_alive, start).await,
            (
                Due::Ping,
                (INTERVAL - Duration::from_secs(1)) * 10 + INTERVAL
            ),
            "pinged an interval after its last frame",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_frame_in_the_same_instant_as_the_ping_is_its_answer() {
        let mut keep_alive = KeepAlive::new(Some(INTERVAL), TIMEOUT);
        assert_eq!(keep_alive.due().await, Due::Ping);
        keep_alive.heard();
        tokio::time::sleep(TIMEOUT * 2).await;
        assert!(
            std::future::poll_fn(|cx| Poll::Ready(keep_alive.poll_due(cx)))
                .await
                .is_pending(),
            "a peer that answered is not given up, and is not pinged again before the interval",
        );
    }

    /// A message heard on another task, read when the wait ends, answers the
    /// ping as one heard in time does: the next ping comes an interval after
    /// it arrived, never an interval after it was read.
    #[tokio::test(start_paused = true)]
    async fn a_message_read_after_it_arrived_counts_from_its_arrival() {
        let start = Instant::now();
        let mut keep_alive = KeepAlive::new(Some(INTERVAL), TIMEOUT);
        assert_eq!(next_due(&mut keep_alive, start).await.0, Due::Ping);

        let answered = start + INTERVAL + Duration::from_secs(1);
        tokio::time::sleep(TIMEOUT).await;
        keep_alive.heard_at(answered);

        assert_eq!(
            next_due(&mut keep_alive, start).await,
            (Due::Ping, INTERVAL + Duration::from_secs(1) + INTERVAL),
        );

        let long_ago = Instant::now() - INTERVAL * 2;
        keep_alive.heard_at(long_ago);
        assert_eq!(
            next_due(&mut keep_alive, start).await,
            (Due::Ping, INTERVAL + Duration::from_secs(1) + INTERVAL),
            "heard longer ago than the interval, the peer is pinged at once",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_interval_of_none_owes_nothing() {
        let mut keep_alive = KeepAlive::new(None, TIMEOUT);
        tokio::select! {
            biased;
            due = keep_alive.due() => panic!("{due:?} owed with the keepalive off"),
            () = tokio::time::sleep(Duration::from_secs(24 * 60 * 60)) => {}
        }
    }
}
