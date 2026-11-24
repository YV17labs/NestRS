//! [`ResilienceEvent`] — what a policy tells its app as it happens, each one
//! the fact a line or a counter of the policy says — read through
//! [`ResilienceEvents`], the subscription
//! [`ResiliencePolicy::events`](crate::ResiliencePolicy::events) opens.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::broadcast;

use crate::breaker::BreakerPhase;
use crate::error::Refusal;
use crate::resilient::ResilientMethod;

/// How many events a subscription holds unread before the oldest are dropped,
/// and counted in a [`ResilienceEvent::Lag`].
const HELD: usize = 256;

/// One thing a policy did to a call, or its breaker did, as
/// [`ResiliencePolicy::events`](crate::ResiliencePolicy::events) tells it.
///
/// Each says what a line of the policy says, in that line's fields: `host` and
/// `method` name the `#[resilient]` method the call came from, as its
/// `mounted` line would — both `None` for a call run through
/// [`run`](crate::ResiliencePolicy::run),
/// [`run_idempotent`](crate::ResiliencePolicy::run_idempotent) or the
/// middleware — and an `error` is the error's chain as the line tells it, a URL
/// cut to its origin, never the error itself.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResilienceEvent {
    /// The run's fallback answers in place of the error it ended on.
    #[non_exhaustive]
    Fallback {
        /// The decorated method's host.
        host: Option<String>,
        /// The decorated method.
        method: Option<&'static str>,
        /// The chain of the error the run ended on: the call's own, from its
        /// last attempt, or the policy's refusal.
        error: String,
    },
    /// The subscription fell behind: the policy told more events than a
    /// subscription holds unread, and the oldest were dropped.
    #[non_exhaustive]
    Lag {
        /// How many events were dropped, the ones before the next it answers.
        missed: u64,
    },
    /// The policy refused a call without sending it.
    #[non_exhaustive]
    Refusal {
        /// The decorated method's host.
        host: Option<String>,
        /// The decorated method.
        method: Option<&'static str>,
        /// Why, as the caller's [`ResilienceError`](crate::ResilienceError)
        /// says it.
        refusal: ResilienceRefusal,
    },
    /// An attempt answered what the run sends its call again after: a failure,
    /// a timeout, or an answer its method said to send again.
    #[non_exhaustive]
    Retry {
        /// The decorated method's host.
        host: Option<String>,
        /// The decorated method.
        method: Option<&'static str>,
        /// Which attempt of the run it was, the first being `1`.
        attempt: u32,
        /// How long the run waits before the next attempt.
        wait: Duration,
        /// What the attempt answered in error: its error's chain, or the
        /// timeout's sentence, and `None` for an `Ok` sent again.
        error: Option<String>,
    },
    /// An attempt was given up at its bound, unanswered. A retry follows it
    /// when the run sends its call again.
    #[non_exhaustive]
    Timeout {
        /// The decorated method's host.
        host: Option<String>,
        /// The decorated method.
        method: Option<&'static str>,
        /// The bound, as [`ResilienceError::TimedOut`](crate::ResilienceError::TimedOut)
        /// says it: none, for a call its unit had no time left to send.
        timeout: Duration,
    },
    /// The circuit breaker changed phase, or was switched by hand. The events
    /// come in the order the transitions took effect, each `from` the `to` of
    /// the one before it.
    #[non_exhaustive]
    Transition {
        /// The phase it left.
        from: BreakerPhase,
        /// The phase it entered: the one it was in already, for a switch by
        /// hand that found it there.
        to: BreakerPhase,
        /// Whether [`open_breaker`](crate::ResiliencePolicy::open_breaker) or
        /// [`close_breaker`](crate::ResiliencePolicy::close_breaker) moved it.
        manual: bool,
    },
}

/// Why a policy refused a call without sending it, in the words
/// [`ResilienceError`](crate::ResilienceError)'s variants say it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResilienceRefusal {
    /// Every slot of the bulkhead stayed taken for as long as the call waits
    /// for one, or as many calls as may wait were waiting.
    Full,
    /// The rate limit had no token free that the call could wait for.
    #[non_exhaustive]
    Limited {
        /// How long until the next token is free.
        retry_after: Duration,
    },
    /// The circuit breaker is open, or another call holds its probe's turn.
    #[non_exhaustive]
    Open {
        /// How long until the breaker lets a call through again.
        retry_after: Duration,
    },
}

impl ResilienceEvent {
    /// `refusal`, told of a call `method` made when a decorated method ran it:
    /// a timeout as the attempt given up, any other as the call refused unsent.
    pub(crate) fn refused(method: Option<&ResilientMethod>, refusal: Refusal) -> Self {
        let (host, method) = named(method);
        let refusal = match refusal {
            Refusal::TimedOut(timeout) => {
                return Self::Timeout {
                    host,
                    method,
                    timeout,
                };
            }
            Refusal::Open(retry_after) => ResilienceRefusal::Open { retry_after },
            Refusal::Full => ResilienceRefusal::Full,
            Refusal::Limited(retry_after) => ResilienceRefusal::Limited { retry_after },
        };
        Self::Refusal {
            host,
            method,
            refusal,
        }
    }

    /// The `attempt`th attempt of a run of `method` sent again after `wait`,
    /// having answered `error`.
    pub(crate) fn retry(
        method: Option<&ResilientMethod>,
        attempt: u32,
        wait: Duration,
        error: Option<&str>,
    ) -> Self {
        let (host, method) = named(method);
        Self::Retry {
            host,
            method,
            attempt,
            wait,
            error: error.map(str::to_owned),
        }
    }

    /// A run of `method` ended on `error`, which its fallback answers for.
    pub(crate) fn fallback(method: &ResilientMethod, error: String) -> Self {
        let (host, method) = named(Some(method));
        Self::Fallback {
            host,
            method,
            error,
        }
    }
}

/// The `host` and `method` an event names a decorated method by.
fn named(method: Option<&ResilientMethod>) -> (Option<String>, Option<&'static str>) {
    (
        method.map(ResilientMethod::host),
        method.map(ResilientMethod::name),
    )
}

/// The subscriptions a policy tells its events to. With none open, an event is
/// neither built nor sent: [`tell`](Self::tell) reads one counter.
#[derive(Debug, Default)]
pub(crate) struct Subscribers {
    open: Arc<AtomicUsize>,
    /// Made at the first subscription, so a policy nothing listens to holds no
    /// buffer.
    sender: OnceLock<broadcast::Sender<ResilienceEvent>>,
}

impl Subscribers {
    /// A subscription to every event told from now on.
    pub(crate) fn subscribe(&self) -> ResilienceEvents {
        let receiver = self
            .sender
            .get_or_init(|| broadcast::Sender::new(HELD))
            .subscribe();
        self.open.fetch_add(1, Ordering::Relaxed);
        ResilienceEvents {
            receiver,
            open: Arc::clone(&self.open),
        }
    }

    /// Whether a subscription is open.
    #[inline]
    pub(crate) fn any(&self) -> bool {
        self.open.load(Ordering::Relaxed) != 0
    }

    /// Tell every subscription the event `said` builds, when one is open.
    #[inline]
    pub(crate) fn tell(&self, said: impl FnOnce() -> ResilienceEvent) {
        if self.any() {
            self.send(said());
        }
    }

    #[inline(never)]
    fn send(&self, event: ResilienceEvent) {
        if let Some(sender) = self.sender.get() {
            // Its one refusal is no receiver left: the last subscription closed
            // since the counter was read.
            let _unheard = sender.send(event);
        }
    }
}

/// A subscription to a policy's events, opened by
/// [`ResiliencePolicy::events`](crate::ResiliencePolicy::events): every event
/// the policy tells from then on, in the order it tells them.
///
/// It holds up to 256 events unread. Past that the oldest are dropped, and
/// [`next`](Self::next) says how many in a [`ResilienceEvent::Lag`] before it
/// answers the oldest one kept. Dropped, it costs its policy nothing more.
#[derive(Debug)]
pub struct ResilienceEvents {
    receiver: broadcast::Receiver<ResilienceEvent>,
    open: Arc<AtomicUsize>,
}

impl ResilienceEvents {
    /// The next event, waited for, or `None` once the policy is dropped and
    /// every event it told was read.
    ///
    /// Dropped while it waits, it loses no event: the next call answers it.
    pub async fn next(&mut self) -> Option<ResilienceEvent> {
        match self.receiver.recv().await {
            Ok(event) => Some(event),
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                Some(ResilienceEvent::Lag { missed })
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }
}

impl Drop for ResilienceEvents {
    fn drop(&mut self) {
        self.open.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;

    fn lag(missed: u64) -> ResilienceEvent {
        ResilienceEvent::Lag { missed }
    }

    /// With no subscription open the event is not built: what builds it — a
    /// chain copied, a host named — never runs.
    #[tokio::test]
    async fn an_event_is_built_only_while_a_subscription_is_open() {
        let subscribers = Subscribers::default();
        let built = AtomicBool::new(false);
        let said = || {
            built.store(true, Ordering::Relaxed);
            lag(0)
        };
        subscribers.tell(said);
        assert!(!built.load(Ordering::Relaxed), "nothing listens yet");

        let mut events = subscribers.subscribe();
        subscribers.tell(said);
        assert_eq!(events.next().await, Some(lag(0)));

        built.store(false, Ordering::Relaxed);
        drop(events);
        subscribers.tell(said);
        assert!(
            !built.load(Ordering::Relaxed),
            "the one subscription closed"
        );
    }

    /// A subscription past what it holds unread is told how many events it
    /// missed, then the oldest kept, and every one after it.
    #[tokio::test]
    async fn a_subscription_that_fell_behind_is_told_how_many_events_it_missed() {
        let subscribers = Subscribers::default();
        let mut events = subscribers.subscribe();
        let told = HELD as u64 + 3;
        for missed in 0..told {
            subscribers.tell(|| lag(missed));
        }
        assert_eq!(
            events.next().await,
            Some(lag(3)),
            "the three oldest, dropped"
        );
        for kept in 3..told {
            assert_eq!(events.next().await, Some(lag(kept)));
        }
    }

    /// Its policy dropped, a subscription answers what was told before, then
    /// ends.
    #[tokio::test]
    async fn a_subscription_ends_once_its_policy_is_dropped_and_its_events_are_read() {
        let subscribers = Subscribers::default();
        let mut events = subscribers.subscribe();
        subscribers.tell(|| lag(7));
        drop(subscribers);
        assert_eq!(events.next().await, Some(lag(7)));
        assert_eq!(events.next().await, None);
    }
}
