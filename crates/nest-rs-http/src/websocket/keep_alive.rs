//! [`KeepAlive`], RFC 6455 §5.5.2's keepalive both socket edges hold: a Ping
//! once the peer has been silent for the interval, and the peer given up when
//! nothing comes back within the timeout of it.

use std::future::Future as _;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::time::{Instant, Sleep};

/// What a socket owes its peer once [`KeepAlive::poll_due`] is ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "a keepalive has these two outcomes, and each socket edge matches both"
)]
pub enum Due {
    /// The peer has been silent for the interval: send it a Ping.
    Ping,
    /// Nothing came back within the timeout of the Ping: the peer is gone, and
    /// the socket is dropped.
    Silent,
}

/// A socket's keepalive: one timer, armed at the instant the next Ping or the
/// end of its wait is due, so a frame from the peer costs a clock read, and the
/// timer moves when it fires or when a Ping is answered.
pub struct KeepAlive(Option<Pinging>);

struct Pinging {
    interval: Duration,
    timeout: Duration,
    timer: Pin<Box<Sleep>>,
    /// When the peer's last frame arrived, or the socket opened.
    heard_at: Instant,
    /// A Ping is out and nothing has come back since.
    awaiting: bool,
}

impl KeepAlive {
    /// A keepalive sending a Ping once the peer is silent for `interval`, and
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

    /// A frame of any kind came from the peer: it is there. The first frame
    /// after a Ping moves the timer to the next Ping's instant, once per Ping,
    /// so a wait past the interval never holds the next one back.
    pub fn heard(&mut self) {
        if let Some(pinging) = &mut self.0 {
            pinging.heard_at = Instant::now();
            if pinging.awaiting {
                pinging.awaiting = false;
                let next = pinging.heard_at + pinging.interval;
                pinging.timer.as_mut().reset(next);
            }
        }
    }

    /// Ready once a Ping is due or the peer let one go unanswered past the
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
