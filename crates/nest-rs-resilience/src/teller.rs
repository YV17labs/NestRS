//! [`Teller`] — what tells what a policy does, each fact in one place: the
//! line it files on [`TARGET`], the counter it counts on and the event it
//! tells, by the instance its policy is named for.

use std::error::Error;
use std::time::Duration;

use crate::TARGET;
use crate::breaker::BreakerPhase;
use crate::counters::Counters;
use crate::error::ResilienceRefusal;
use crate::events::{ResilienceEvent, ResilienceEvents, Subscribers};
use crate::millis::millis;
use crate::resilient::ResilientMethod;
use crate::told::told;

// The message of each line a policy files on `TARGET`, its breaker's transition
// apart: what happened, then what the call does, which names the constant.

/// A retry, certain, after an attempt that timed out or answered an `Err`.
pub(crate) const RETRYING_FAILED: &str = "attempt failed; retrying the call";

/// A retry, certain, after an attempt that answered an `Ok` sent again.
pub(crate) const RETRYING_ANSWERED: &str = "attempt answered; retrying the call";

/// A retry given up: the dependency asked a wait past the backoff's ceiling.
pub(crate) const NOT_ATTEMPTED_AGAIN_PAST_CEILING: &str =
    "the dependency asked a wait past the backoff's ceiling; not attempted again";

/// A retry given up: its wait ends past the unit's deadline. Its `error` is
/// what the attempt failed on, an `Err`'s chain or the timeout's sentence, and
/// it has none after an `Ok` that was to be sent again.
pub(crate) const NOT_ATTEMPTED_AGAIN_PAST_DEADLINE: &str =
    "no time left before the unit's deadline; not attempted again";

/// A run ended on its attempt's savepoint, which the database could not settle.
pub(crate) const NOT_ATTEMPTED_AGAIN_UNSETTLED: &str =
    "an attempt's savepoint could not be settled; not attempted again";

/// An attempt waiting for the token of the rate it reserved.
pub(crate) const WAITING_FOR_TOKEN: &str = "rate limit reached; waiting for a token";

/// A call the bulkhead refuses ([`ResilienceRefusal::Full`]).
pub(crate) const REFUSED_FULL: &str = "bulkhead full; the call is refused";

/// A call the rate refuses ([`ResilienceRefusal::Limited`]).
pub(crate) const REFUSED_LIMITED: &str = "rate limit reached; the call is refused";

/// A fallback answering in place of the error its run ended on.
pub(crate) const FALLBACK_ANSWERS: &str = "the call ended on an error; its fallback answers";

/// A breaker's transition, the one line a policy files at `warn`.
pub(crate) const MOVED: &str = "circuit breaker changed state";

/// What tells one policy's facts: its lines, named by its instance, its
/// counters and its subscriptions. A line naming a call names the decorated
/// method it came from, read only when the line is filed.
#[derive(Debug)]
pub(crate) struct Teller {
    instance: &'static str,
    counters: Counters,
    subscribers: Subscribers,
}

/// One transition a breaker made, as its line, the counter and its event say
/// it once the breaker's lock is let go.
#[derive(Debug)]
pub(crate) struct Transition {
    pub(crate) from: BreakerPhase,
    pub(crate) to: BreakerPhase,
    /// The failures that opened it and the window that counted them, when
    /// failures did.
    pub(crate) failed: Option<(usize, usize)>,
    /// What the last of them failed on.
    pub(crate) error: Option<Box<str>>,
    /// The time it is open for, when it opened for one of its own.
    pub(crate) open_for: Option<Duration>,
    pub(crate) manual: bool,
}

/// `instance` as a line or a series names it: by its marker's name, and not at
/// all for the default instance, whose name is empty — a field holding nothing
/// is left out.
pub(crate) const fn named(instance: &'static str) -> Option<&'static str> {
    if instance.is_empty() {
        None
    } else {
        Some(instance)
    }
}

/// The `host` a line names a decorated method's call by.
fn host(method: Option<&ResilientMethod>) -> Option<String> {
    method.map(ResilientMethod::host)
}

/// The `method` a line names a decorated method's call by.
fn name(method: Option<&ResilientMethod>) -> Option<&'static str> {
    method.map(ResilientMethod::name)
}

impl Teller {
    /// What tells the facts of the policy of `instance`, counted on the
    /// meter its counters find at their first record.
    pub(crate) fn new(instance: &'static str) -> Self {
        Self::counting_on(instance, Counters::new(instance))
    }

    /// What tells the facts of the policy of `instance`, counted on
    /// `counters`.
    pub(crate) fn counting_on(instance: &'static str, counters: Counters) -> Self {
        Self {
            instance,
            counters,
            subscribers: Subscribers::default(),
        }
    }

    /// The instance the policy is named for, as its refusals carry it: empty
    /// for the default instance.
    pub(crate) const fn instance(&self) -> &'static str {
        self.instance
    }

    /// The `instance` of every line, none for the default instance.
    const fn named(&self) -> Option<&'static str> {
        named(self.instance)
    }

    /// A subscription to every event told from now on.
    pub(crate) fn subscribe(&self) -> ResilienceEvents {
        self.subscribers.subscribe()
    }

    /// `refusal`, which a run of `method` ends on, counted and told. It files
    /// no line: the bulkhead's and the rate's file theirs where they refuse.
    pub(crate) fn refused(
        &self,
        method: Option<&ResilientMethod>,
        refusal: ResilienceRefusal,
    ) -> ResilienceRefusal {
        self.counters.refusal(refusal);
        self.subscribers
            .tell(|| ResilienceEvent::refused(method, refusal));
        refusal
    }

    /// An attempt of a run of `method` given up at its bound, which the run
    /// sends its call again after, told: its run ends on no refusal, where
    /// one is told otherwise ([`refused`](Self::refused)).
    pub(crate) fn given_up(&self, method: Option<&ResilientMethod>, timed_out: ResilienceRefusal) {
        self.subscribers
            .tell(|| ResilienceEvent::refused(method, timed_out));
    }

    /// The line of a call of `method` the bulkhead refuses, naming the
    /// `waiters` it met when as many calls waited as may.
    pub(crate) fn full(&self, method: Option<&ResilientMethod>, waiters: Option<usize>) {
        tracing::debug!(
            target: TARGET,
            instance = self.named(),
            host = host(method).as_deref(),
            method = name(method),
            waiters,
            "{REFUSED_FULL}",
        );
    }

    /// The line of a call of `method` the rate refuses, `wait` the time to
    /// the next token free.
    pub(crate) fn limited(&self, method: Option<&ResilientMethod>, wait: Duration) {
        tracing::debug!(
            target: TARGET,
            instance = self.named(),
            host = host(method).as_deref(),
            method = name(method),
            retry_after_ms = millis(wait),
            "{REFUSED_LIMITED}",
        );
    }

    /// The line of an attempt of `method` waiting `wait` for the token of
    /// the rate it reserved.
    pub(crate) fn waiting_for_token(&self, method: Option<&ResilientMethod>, wait: Duration) {
        tracing::debug!(
            target: TARGET,
            instance = self.named(),
            host = host(method).as_deref(),
            method = name(method),
            wait_ms = millis(wait),
            "{WAITING_FOR_TOKEN}",
        );
    }

    /// Whether anything hears the facts a retry tells, which say an error's
    /// chain: a subscription open, or a line at `debug` on [`TARGET`]
    /// filed. Read where the retry is decided, so an error is put in words
    /// only for what hears it.
    pub(crate) fn hears(&self) -> bool {
        self.subscribers.any() || tracing::enabled!(target: TARGET, tracing::Level::DEBUG)
    }

    /// A call sent again, counted as it is sent.
    pub(crate) fn resent(&self) {
        self.counters.retry();
    }

    /// The line of a retry of `method` that is certain, saying what the
    /// `attempt` before it answered: `attempt failed`, with `error`, an
    /// `Err`'s chain or the timeout's sentence, whichever way the `Err`
    /// counts in the breaker, and `attempt answered`, with no error, for an
    /// `Ok` sent again. The retry is told as it is filed.
    pub(crate) fn retrying(
        &self,
        method: Option<&ResilientMethod>,
        attempt: u32,
        wait: Duration,
        error: Option<&str>,
    ) {
        match error {
            Some(error) => tracing::debug!(
                target: TARGET,
                instance = self.named(),
                host = host(method).as_deref(),
                method = name(method),
                attempt,
                wait_ms = millis(wait),
                error,
                "{RETRYING_FAILED}",
            ),
            None => tracing::debug!(
                target: TARGET,
                instance = self.named(),
                host = host(method).as_deref(),
                method = name(method),
                attempt,
                wait_ms = millis(wait),
                "{RETRYING_ANSWERED}",
            ),
        }
        self.subscribers
            .tell(|| ResilienceEvent::retry(method, attempt, wait, error));
    }

    /// The line of a retry of `method` given up, the dependency having
    /// `asked` a wait past the backoff's `ceiling` after an attempt that
    /// answered `error`.
    pub(crate) fn past_ceiling(
        &self,
        method: Option<&ResilientMethod>,
        asked: Duration,
        ceiling: Duration,
        error: Option<&str>,
    ) {
        tracing::debug!(
            target: TARGET,
            instance = self.named(),
            host = host(method).as_deref(),
            method = name(method),
            asked_ms = millis(asked),
            ceiling_ms = millis(ceiling),
            error,
            "{NOT_ATTEMPTED_AGAIN_PAST_CEILING}",
        );
    }

    /// The line of a retry of `method` given up, its `wait` ending past the
    /// unit's deadline, after an attempt that answered `error`: none, after
    /// an `Ok` that was to be sent again.
    pub(crate) fn past_deadline(
        &self,
        method: Option<&ResilientMethod>,
        wait: Duration,
        error: Option<&str>,
    ) {
        tracing::debug!(
            target: TARGET,
            instance = self.named(),
            host = host(method).as_deref(),
            method = name(method),
            wait_ms = millis(wait),
            error,
            "{NOT_ATTEMPTED_AGAIN_PAST_DEADLINE}",
        );
    }

    /// The line of a run of `method` ended on its attempt's savepoint, which
    /// `cause` left unsettled, the cause's chain as its `error`: a driver
    /// owes no line of its own for it.
    pub(crate) fn unsettled(
        &self,
        method: Option<&ResilientMethod>,
        cause: &(dyn Error + 'static),
    ) {
        tracing::debug!(
            target: TARGET,
            instance = self.named(),
            host = host(method).as_deref(),
            method = name(method),
            error = told(cause).as_str(),
            "{NOT_ATTEMPTED_AGAIN_UNSETTLED}",
        );
    }

    /// A run of `method` ended on `error`, which its fallback answers in
    /// place of: the line saying so, with the error's chain, counted and
    /// told. The chain is said once, and only for a line filed or a
    /// subscription open.
    pub(crate) fn fallback(&self, method: &ResilientMethod, error: &(dyn Error + 'static)) {
        let mut said = None;
        tracing::debug!(
            target: TARGET,
            instance = self.named(),
            host = method.host(),
            method = method.name(),
            error = said.insert(told(error)).as_str(),
            "{FALLBACK_ANSWERS}",
        );
        self.counters.fallback();
        self.subscribers
            .tell(|| ResilienceEvent::fallback(method, said.unwrap_or_else(|| told(error))));
    }

    /// `transition`'s line, its count and its event, told with no lock of its
    /// breaker held: a line runs the app's `tracing` subscriber, a counter
    /// waits for the meter's pipeline at its first record, and an event wakes
    /// its readers, any of which may read or switch that breaker.
    pub(crate) fn transition(&self, transition: &Transition) {
        let Transition {
            from,
            to,
            failed,
            ref error,
            open_for,
            manual,
        } = *transition;
        let state = to.as_str();
        tracing::warn!(
            target: TARGET,
            instance = self.named(),
            state,
            failed = failed.map(|(failed, _)| failed),
            window = failed.map(|(_, window)| window),
            open_ms = open_for.map(millis),
            manual = manual.then_some(true),
            error = error.as_deref(),
            "{MOVED}",
        );
        self.counters.transition(state, manual);
        self.subscribers
            .tell(|| ResilienceEvent::Transition { from, to, manual });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resilient::resilient_method;

    #[derive(Debug, thiserror::Error)]
    #[error("the dependency is down")]
    struct Down;

    /// Each line the teller of `instance` files, once.
    fn tell_each(instance: &'static str) -> usize {
        const TRACK: ResilientMethod = resilient_method(|| "ShipmentsService", "track", None, None);
        let teller = Teller::new(instance);
        let wait = Duration::from_millis(100);
        teller.full(Some(&TRACK), Some(1));
        teller.limited(Some(&TRACK), wait);
        teller.waiting_for_token(Some(&TRACK), wait);
        teller.retrying(Some(&TRACK), 1, wait, Some("down"));
        teller.retrying(Some(&TRACK), 1, wait, None);
        teller.past_ceiling(Some(&TRACK), wait, wait, Some("down"));
        teller.past_deadline(Some(&TRACK), wait, None);
        teller.unsettled(Some(&TRACK), &Down);
        teller.fallback(&TRACK, &Down);
        teller.transition(&Transition {
            from: BreakerPhase::Closed,
            to: BreakerPhase::Open,
            failed: Some((1, 1)),
            error: Some("down".into()),
            open_for: Some(wait),
            manual: false,
        });
        10
    }

    /// A named instance's lines say it as `instance`; the default instance,
    /// named by nothing, files lines with no `instance`, as a field holding
    /// nothing is left out everywhere else.
    #[test]
    fn every_line_names_a_named_instance_and_the_default_s_name_none() {
        let logs = nest_rs_testing::LogCapture::install();
        let lines = || -> Vec<_> {
            logs.events()
                .into_iter()
                .filter(|line| line.target == TARGET)
                .collect()
        };
        let told = tell_each("upstream");
        let named = lines();
        assert_eq!(named.len(), told, "{named:#?}");
        for line in &named {
            assert_eq!(
                line.field("instance").as_deref(),
                Some("upstream"),
                "{line:?}"
            );
        }

        let told = tell_each("");
        let unnamed = lines().split_off(named.len());
        assert_eq!(unnamed.len(), told, "{unnamed:#?}");
        for line in &unnamed {
            assert_eq!(line.field("instance"), None, "{line:?}");
        }
    }
}
