//! [`ResilienceError`] — what a policy answers when its call does not succeed:
//! the call's own error, or the policy's refusal, which every edge answers
//! `503` with the wait it knows — and [`BreakerError`], what a manual switch
//! of its breaker answers when it changes nothing.

use std::time::Duration;

use nest_rs_core::{Problem, ToProblem, code};
use nest_rs_database::AtomicError;

use crate::config::{LONGEST_OPEN, ResilienceConfig};

const UNAVAILABLE: Problem = Problem::new(503, code::UNAVAILABLE);

/// What an attempt given up at its bound says: [`ResilienceError::TimedOut`]'s
/// sentence, and the `error` of the lines telling that attempt.
pub(crate) const TIMED_OUT: &str = "the call did not answer within its timeout";

/// Why a policy's call did not succeed.
///
/// [`Failed`](Self::Failed) carries the call's own error, from its last
/// attempt, and [`Unsettled`](Self::Unsettled) the failure of the unit's own
/// database. Every other variant is the policy's refusal, answered on every edge
/// as a `503`, `UNAVAILABLE`, an open breaker's and a reached rate limit's with
/// the `Retry-After` each knows. The problem also rides the error's chain: a
/// domain error wrapping this one answers it by handing its
/// [`to_problem`](ToProblem::to_problem) on, and a queue job failing with it
/// waits at least that long before its retry, up to an hour.
///
/// It answers an edge when the call's error does, through [`ToProblem`], so the
/// call's error says what a client of its failure is told — nothing, for a
/// failure no client should read:
///
/// ```
/// use std::time::Duration;
///
/// use nest_rs_core::{Problem, ToProblem, code};
/// use nest_rs_resilience::{ResilienceError, ResiliencePolicy};
///
/// #[derive(Debug, thiserror::Error)]
/// #[error("the payment provider failed")]
/// struct ProviderDown;
///
/// impl ToProblem for ProviderDown {
///     fn to_problem(&self) -> Option<Problem> {
///         None
///     }
/// }
///
/// # async fn charge(policy: &ResiliencePolicy) {
/// match policy.run(|_cancel| async { Ok::<_, ProviderDown>("charged") }).await {
///     Ok(receipt) => assert_eq!(receipt, "charged"),
///     Err(ResilienceError::Open { retry_after, .. }) => {
///         assert!(retry_after > Duration::ZERO);
///     }
///     Err(refused) => {
///         let problem = refused.to_problem();
///         assert!(problem.is_none_or(|problem| problem.code() == code::UNAVAILABLE));
///     }
/// }
/// # }
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ResilienceError<E> {
    /// The call answered its own error on its last attempt. Every edge answers
    /// it as the error's own [`to_problem`](ToProblem::to_problem) says, else
    /// by a problem its chain carries, opaquely when neither does.
    #[error("the call failed")]
    Failed(#[source] E),
    /// The last attempt did not answer within the policy's timeout, or the
    /// unit's deadline came first.
    #[error("{}", TIMED_OUT)]
    #[non_exhaustive]
    TimedOut {
        /// What the attempt was given up at: the policy's timeout — the one a
        /// `#[resilient]` method pins, for its own runs — or what was left of
        /// the unit's deadline when that was shorter: none, for a call the unit
        /// had no time left to send.
        timeout: Duration,
        #[doc(hidden)]
        #[source]
        __problem: Problem,
    },
    /// The circuit breaker is open: the call was refused without being sent.
    #[error("the circuit breaker is open")]
    #[non_exhaustive]
    Open {
        /// How long until the breaker lets a call through again, at the
        /// earliest — or, while another call probes the dependency, until that
        /// probe has answered at the latest: the `Retry-After` every edge
        /// answers, in whole seconds rounded up.
        retry_after: Duration,
        #[doc(hidden)]
        #[source]
        __problem: Problem,
    },
    /// Every slot of the bulkhead stayed taken for as long as the call waits
    /// for one, or as many calls as may wait for one were waiting: the call
    /// was refused without being sent. The error does not say which of the
    /// two; the refusal's line does, by its `waiters`.
    #[error("the bulkhead has no free slot")]
    #[non_exhaustive]
    Full {
        #[doc(hidden)]
        #[source]
        __problem: Problem,
    },
    /// The rate limit had no token free for the attempt, and the next one due
    /// it, behind the attempts already waiting for theirs, came past what it
    /// could wait or past its unit's deadline: the call was refused without
    /// being sent, and is not attempted again.
    #[error("the rate limit is reached")]
    #[non_exhaustive]
    Limited {
        /// How long until that token is free: the `Retry-After` every edge
        /// answers, in whole seconds rounded up.
        retry_after: Duration,
        #[doc(hidden)]
        #[source]
        __problem: Problem,
    },
    /// An attempt ran in a savepoint of its unit's transaction, and the
    /// database could not settle it: keep what the attempt wrote once it
    /// answered, or roll it back once it failed. The run ends there, whatever
    /// the attempt answered, and is not attempted again: nothing that attempt
    /// wrote stands. No refusal of the policy's, so every edge answers it as
    /// the server's own failure, opaquely, and no fallback answers in its
    /// place.
    #[error("an attempt's savepoint could not be settled")]
    #[non_exhaustive]
    Unsettled {
        #[doc(hidden)]
        #[source]
        __cause: AtomicError,
    },
}

impl<E> ResilienceError<E> {
    /// The run ended on its attempt's savepoint, which `cause` left unsettled.
    pub(crate) const fn unsettled(cause: AtomicError) -> Self {
        Self::Unsettled { __cause: cause }
    }
}

/// Why [`ResiliencePolicy::open_breaker`](crate::ResiliencePolicy::open_breaker)
/// or [`close_breaker`](crate::ResiliencePolicy::close_breaker) left the breaker
/// as it was: a manual switch is never a silent no-op, nor clamped to what it
/// can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BreakerError {
    /// The policy has no circuit breaker, its window set to none.
    #[error("the policy has no circuit breaker to switch")]
    Off,
    /// The duration is outside the open times
    /// [`breaker_open`](ResilienceConfig::breaker_open) takes in code, the
    /// range a manual opening is held to: none, or past its ceiling.
    #[error(
        "a breaker opened by hand stays open for a time above zero, up to {most:?}, the open \
         times `{}` takes in code",
        ResilienceConfig::BREAKER_OPEN.field().path()
    )]
    #[non_exhaustive]
    OutOfRange {
        /// The longest open time a manual opening takes, the ceiling of
        /// [`breaker_open`](ResilienceConfig::breaker_open).
        most: Duration,
    },
}

impl BreakerError {
    /// A duration outside the open times `breaker_open` takes in code.
    pub(crate) const fn out_of_range() -> Self {
        Self::OutOfRange { most: LONGEST_OPEN }
    }
}

/// A policy's refusal, before it is told in the caller's error type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    TimedOut(Duration),
    /// For this long at least.
    Open(Duration),
    Full,
    /// Until the token the attempt would be due is free.
    Limited(Duration),
}

impl Refusal {
    /// The word the refusals' counter says.
    #[cfg(feature = "opentelemetry")]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::TimedOut(_) => "timed_out",
            Self::Open(_) => "open",
            Self::Full => "full",
            Self::Limited(_) => "limited",
        }
    }
}

impl<E> From<Refusal> for ResilienceError<E> {
    fn from(refusal: Refusal) -> Self {
        use nest_rs_core::__private::retry_after_secs;

        match refusal {
            Refusal::TimedOut(timeout) => Self::TimedOut {
                timeout,
                __problem: UNAVAILABLE,
            },
            Refusal::Open(retry_after) => Self::Open {
                retry_after,
                __problem: UNAVAILABLE.with_retry_after(retry_after_secs(retry_after)),
            },
            Refusal::Full => Self::Full {
                __problem: UNAVAILABLE,
            },
            Refusal::Limited(retry_after) => Self::Limited {
                retry_after,
                __problem: UNAVAILABLE.with_retry_after(retry_after_secs(retry_after)),
            },
        }
    }
}

impl<E: ToProblem> ToProblem for ResilienceError<E> {
    fn to_problem(&self) -> Option<Problem> {
        let carried = match self {
            Self::Failed(error) => match error.to_problem() {
                Some(said) => return Some(said),
                None => nest_rs_core::__private::find_in_chain::<Problem>(error),
            },
            refused => nest_rs_core::__private::find_in_chain::<Problem>(refused),
        };
        carried.cloned()
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::error_message;

    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("the upstream answered 502")]
    struct Upstream;

    impl ToProblem for Upstream {
        fn to_problem(&self) -> Option<Problem> {
            None
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("the upstream asked to slow down")]
    struct Passed(#[source] Problem);

    impl ToProblem for Passed {
        fn to_problem(&self) -> Option<Problem> {
            None
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("no parcel by that number")]
    struct NoParcel;

    impl ToProblem for NoParcel {
        fn to_problem(&self) -> Option<Problem> {
            Some(Problem::new(404, code::NOT_FOUND))
        }
    }

    #[test]
    fn every_refusal_is_a_503_and_an_open_breaker_and_a_reached_rate_say_their_wait_rounded_up() {
        let refusals: [ResilienceError<Upstream>; 4] = [
            Refusal::TimedOut(Duration::from_secs(5)).into(),
            Refusal::Open(Duration::from_millis(2_500)).into(),
            Refusal::Full.into(),
            Refusal::Limited(Duration::from_millis(40_200)).into(),
        ];
        let answered: Vec<_> = refusals
            .iter()
            .map(|refused| {
                let problem = refused.to_problem().expect("a refusal answers");
                (problem.status(), problem.code(), problem.retry_after())
            })
            .collect();
        assert_eq!(
            answered,
            [
                (503, code::UNAVAILABLE, None),
                (503, code::UNAVAILABLE, Some(3)),
                (503, code::UNAVAILABLE, None),
                (503, code::UNAVAILABLE, Some(41)),
            ]
        );
    }

    /// An error wrapping the refusal answers it too: the problem rides the
    /// chain every edge and the queue read.
    #[test]
    fn the_refusals_problem_rides_its_chain() {
        let refused: ResilienceError<Upstream> = Refusal::Open(Duration::from_secs(30)).into();
        let found = nest_rs_core::__private::find_in_chain::<Problem>(&refused)
            .expect("the problem is in the chain");
        assert_eq!(found.retry_after(), Some(30));
        assert_eq!(
            error_message(&refused),
            "the circuit breaker is open: 503 UNAVAILABLE"
        );

        let limited: ResilienceError<Upstream> = Refusal::Limited(Duration::from_secs(12)).into();
        let found = nest_rs_core::__private::find_in_chain::<Problem>(&limited)
            .expect("the problem is in the chain");
        assert_eq!(
            (found.status(), found.code(), found.retry_after()),
            (503, code::UNAVAILABLE, Some(12))
        );
        assert_eq!(
            error_message(&limited),
            "the rate limit is reached: 503 UNAVAILABLE"
        );
    }

    #[test]
    fn a_failed_call_answers_the_problem_its_chain_carries_and_else_nothing() {
        let opaque = ResilienceError::Failed(Upstream);
        assert_eq!(opaque.to_problem(), None, "the edge answers it opaquely");
        assert_eq!(
            error_message(&opaque),
            "the call failed: the upstream answered 502"
        );

        let asked = Problem::new(429, code::RATE_LIMITED).with_retry_after(7);
        let passed = ResilienceError::Failed(Passed(asked.clone()));
        assert_eq!(passed.to_problem(), Some(asked));
    }

    /// A savepoint the database could not settle is the server's failure: it
    /// answers no problem of the policy's, and its chain says what the port
    /// said.
    #[test]
    fn an_unsettled_savepoint_answers_opaquely_and_says_its_cause() {
        let unsettled: ResilienceError<Upstream> =
            ResilienceError::unsettled(AtomicError::Poisoned { retryable: false });
        assert_eq!(unsettled.to_problem(), None, "the edge answers it opaquely");
        assert_eq!(
            error_message(&unsettled),
            "an attempt's savepoint could not be settled: a statement failed inside the atomic \
             block; nothing it wrote stands"
        );
    }

    /// The call's error is read as itself before its chain: a domain error's
    /// own problem is never lost under the policy.
    #[test]
    fn a_failed_call_answers_the_problem_its_error_says() {
        let said = ResilienceError::Failed(NoParcel).to_problem();
        assert_eq!(said, Some(Problem::new(404, code::NOT_FOUND)));
    }
}
