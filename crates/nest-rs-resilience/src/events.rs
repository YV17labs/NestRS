//! [`ResilienceEvent`] — what a policy tells its app as it happens: each
//! retry, each attempt given up at its bound, each call refused unsent, each
//! transition of its breaker and each fallback answering — read through
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
/// An event names its call as the policy's lines do: `host` and `method` are
/// the `#[resilient]` method the call came from, as its `mounted` line would
/// name it. Both are `None` for a call run through
/// [`run`](crate::ResiliencePolicy::run),
/// [`run_idempotent`](crate::ResiliencePolicy::run_idempotent) or the
/// middleware, which take no name: two callers sharing a policy through them
/// are told apart by no event, so decorate the method making the call to name
/// it. An `error` is the error's chain as the lines tell it, a URL cut to its
/// origin, never the error itself.
///
/// An event is not its line. Where a fact files one, the event may say less:
/// a [`Transition`](Self::Transition) not what opened the breaker, a full
/// bulkhead's refusal not the waiters it met. And a call an open breaker
/// refuses, or an attempt given up at its bound, is told with no line of its
/// own.
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
    /// An attempt answered what the run decides to send its call again after:
    /// a failure, a timeout, or an answer its method said to send again. It is
    /// told as the retry is decided, before its wait: once the wait ends the
    /// policy may still refuse the call unsent — a breaker that opened, a rate
    /// with no token — which a [`Refusal`](Self::Refusal) then tells, and the
    /// retries' counter counts the calls sent again alone.
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
    /// when the run sends its call again, and it counts among the refusals on
    /// the meter only when the run ends on it.
    ///
    /// One timeout is told by no event. A response of
    /// [`ResilienceMiddleware`](crate::ResilienceMiddleware) its caller drops
    /// while a read of its body has waited past the bound counts as a timeout
    /// in the breaker, the dependency having delivered nothing in time, and
    /// no caller is left to answer: nothing tells it and no refusal counts
    /// it, but the breaker's [`Transition`](Self::Transition) when that
    /// failure opens it.
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
    /// the one before it. What opened it — the failures its window counted,
    /// what the last failed on, the open time — is on the transition's `warn`
    /// alone.
    ///
    /// A transition is told once the breaker has made it and is free again,
    /// so a reader may read or switch the breaker as it is told — and a call
    /// on another thread may meet the new phase first: its
    /// [`Refusal`](Self::Refusal) can come before the transition that
    /// refused it.
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
    /// The bulkhead took the call into no slot, for one of two causes the
    /// event does not tell apart: every slot stayed taken for as long as the
    /// call waits for one — at once, with no wait — or every place to wait
    /// was taken, `CONCURRENCY_WAITERS` calls waiting already, which the
    /// `waiters` of the refusal's line alone says. It carries no count: the
    /// calls running and waiting are read on the policy, as they stand at the
    /// read and not as the refusal met them, through
    /// [`concurrency_running`](crate::ResiliencePolicy::concurrency_running)
    /// and
    /// [`concurrency_waiting`](crate::ResiliencePolicy::concurrency_waiting).
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
        /// How long until the breaker lets a call through again, as
        /// [`ResilienceError::Open`](crate::ResilienceError::Open) says it.
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
