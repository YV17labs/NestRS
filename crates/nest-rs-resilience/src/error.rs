//! [`ResilienceError`] — what a policy answers when its call does not succeed:
//! the call's own error, or the policy's refusal, which every edge answers
//! `503` with the wait it knows.

use std::error::Error;
use std::time::Duration;

use nest_rs_core::{Problem, ToProblem, code};

/// What every refusal of a policy answers: `503`, `UNAVAILABLE` — the server
/// could not serve the unit because something it depends on did not answer.
const UNAVAILABLE: Problem = Problem::new(503, code::UNAVAILABLE);

/// Why a policy's call did not succeed.
///
/// [`Failed`](Self::Failed) carries the call's own error, from its last
/// attempt. Every other variant is the policy's refusal, answered on every edge
/// as a `503`, `UNAVAILABLE`, an open breaker's with the `Retry-After` it
/// knows. The problem also rides the error's chain: a domain error wrapping
/// this one answers it by handing its [`to_problem`](ToProblem::to_problem)
/// on, and a queue job failing with it waits at least that long before its
/// retry.
///
/// ```
/// use std::time::Duration;
///
/// use nest_rs_core::{ToProblem, code};
/// use nest_rs_resilience::{ResilienceError, ResiliencePolicy};
///
/// # async fn charge(policy: &ResiliencePolicy) {
/// match policy.run(|_cancel| async { Ok::<_, std::io::Error>("charged") }).await {
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
    /// it by the problem its chain carries, opaquely when it carries none.
    #[error("the call failed")]
    Failed(#[source] E),
    /// The last attempt did not answer within the policy's timeout.
    #[error("the call did not answer within its timeout")]
    #[non_exhaustive]
    TimedOut {
        /// The timeout the attempt was given up at.
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
        /// earliest: the `Retry-After` every edge answers, in whole seconds
        /// rounded up.
        retry_after: Duration,
        #[doc(hidden)]
        #[source]
        __problem: Problem,
    },
    /// Every slot of the bulkhead stayed taken for as long as the call waits
    /// for one: the call was refused without being sent.
    #[error("the bulkhead has no free slot")]
    #[non_exhaustive]
    Full {
        #[doc(hidden)]
        #[source]
        __problem: Problem,
    },
}

/// A policy's refusal, before it is told in the caller's error type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The attempt did not answer within this timeout.
    TimedOut(Duration),
    /// The breaker refused, for this long at least.
    Open(Duration),
    /// The bulkhead had no free slot in time.
    Full,
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
        }
    }
}

impl<E: Error + Send + Sync + 'static> ToProblem for ResilienceError<E> {
    fn to_problem(&self) -> Option<Problem> {
        match self {
            Self::Failed(error) => {
                nest_rs_core::__private::find_in_chain::<Problem>(error).cloned()
            }
            Self::TimedOut { __problem, .. }
            | Self::Open { __problem, .. }
            | Self::Full { __problem } => Some(__problem.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::error_message;

    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("the upstream answered 502")]
    struct Upstream;

    #[derive(Debug, thiserror::Error)]
    #[error("the upstream asked to slow down")]
    struct Passed(#[source] Problem);

    #[test]
    fn every_refusal_is_a_503_and_an_open_breaker_says_its_wait_rounded_up() {
        let refusals: [ResilienceError<Upstream>; 3] = [
            Refusal::TimedOut(Duration::from_secs(5)).into(),
            Refusal::Open(Duration::from_millis(2_500)).into(),
            Refusal::Full.into(),
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
    }

    #[test]
    fn a_failed_call_answers_the_problem_its_own_chain_carries_and_nothing_else() {
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
}
