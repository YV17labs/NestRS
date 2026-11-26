//! [`ResilienceError`] — what a policy answers when its call does not succeed:
//! the call's own error, or the policy's refusal, one of [`ResilienceRefusal`],
//! which every edge answers `503` with the wait it knows — and
//! [`BreakerError`], what a manual switch of its breaker answers when it
//! changes nothing.

use std::convert::Infallible;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{Problem, SafeLink, ToProblem, code};
use nest_rs_database::AtomicError;

use crate::config::{LONGEST_OPEN, ResilienceConfig};

const UNAVAILABLE: Problem = Problem::new(503, code::UNAVAILABLE);

/// What an attempt given up at its bound says: the `error` of the lines telling
/// that attempt, which name its policy's instance beside it, and the sentence of
/// [`ResilienceRefusal::TimedOut`] where the default instance's policy refused.
pub(crate) const TIMED_OUT: &str = "the call did not answer within its timeout";

/// What a call answering its own error is said to have done, and what an
/// attempt judged a failure fails on where its call names no error.
pub(crate) const FAILED: &str = "the call failed";

/// What an attempt whose call unwound failed on, or words saying its error
/// that panicked; its payload is its unit's to file.
pub(crate) const PANICKED: &str = "the call panicked";

/// Why a policy's call did not succeed.
///
/// [`Failed`](Self::Failed) carries the call's own error, from its last
/// attempt, and [`Unsettled`](Self::Unsettled) the failure of the unit's own
/// database. [`Refused`](Self::Refused) is the policy's refusal, one of the set
/// [`ResilienceRefusal`] names, answered on every edge as a `503`,
/// `UNAVAILABLE`, an open breaker's and a reached rate limit's with the
/// `Retry-After` each knows. The problem also rides the error's chain: a
/// domain error wrapping this one answers it by handing its
/// [`to_problem`](ToProblem::to_problem) on, and a queue job failing with it
/// waits at least that long before its retry, up to an hour.
///
/// A refusal names the policy that made it by the instance that policy is named
/// for, its `instance`, and so does its sentence, which every line filing the
/// error's chain says: of one handler calling two dependencies, the one that
/// refused. The default instance names none. No reply says it: the problem is
/// the same whichever policy refused, and names no mechanism.
///
/// It is `Clone` when the call's error is, a refusal's problem and an unsettled
/// savepoint's cause shared between the clones: a `#[dataloader]` batch run
/// under a policy answers it as its loader's error, which async-graphql hands
/// each key of the batch a clone of.
///
/// It answers an edge when the call's error does, through [`ToProblem`], so the
/// call's error says what a client of its failure is told — nothing, for a
/// failure no client should read:
///
/// ```
/// use std::time::Duration;
///
/// use nest_rs_core::{Problem, ToProblem, code};
/// use nest_rs_resilience::{ResilienceError, ResiliencePolicy, ResilienceRefusal};
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
/// match policy.run(|| async { Ok::<_, ProviderDown>("charged") }).await {
///     Ok(receipt) => assert_eq!(receipt, "charged"),
///     Err(ResilienceError::Refused {
///         refusal: ResilienceRefusal::Open { retry_after, .. },
///         ..
///     }) => {
///         assert!(retry_after > Duration::ZERO);
///     }
///     Err(refused) => {
///         let problem = refused.to_problem();
///         assert!(problem.is_none_or(|problem| problem.code() == code::UNAVAILABLE));
///     }
/// }
/// # }
/// ```
// A refusal's problem is shared, never inline: inline beside a duration and the
// instance, it takes the error to 128 bytes, where clippy's `result_large_err`
// warns at every method answering it.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ResilienceError<E> {
    /// The call answered its own error on its last attempt. Every edge answers
    /// it as the error's own [`to_problem`](ToProblem::to_problem) says, else
    /// by a problem its chain carries, opaquely when neither does: a partner's
    /// answer that did not decode is the server's failure on every edge, its
    /// chain filed.
    Failed(E),
    /// The policy answered in place of the call: its last attempt given up at
    /// its bound, or the call refused unsent.
    #[non_exhaustive]
    Refused {
        /// Which mechanism refused, and the wait it knows.
        refusal: ResilienceRefusal,
        /// The instance of the policy that refused, as its marker's
        /// [`NAME`](nest_rs_core::Instance::NAME) says it: empty for the
        /// default one.
        instance: &'static str,
        #[doc(hidden)]
        __problem: Arc<Problem>,
    },
    /// An attempt ran in a savepoint of its unit's transaction, and the
    /// database could not settle it: keep what the attempt wrote once it
    /// answered, or roll it back once it failed. The run ends there, whatever
    /// the attempt answered, and is not attempted again: nothing that attempt
    /// wrote stands. No refusal of the policy's, so every edge answers it as
    /// the server's own failure, opaquely, and no fallback answers in its
    /// place.
    #[non_exhaustive]
    Unsettled {
        #[doc(hidden)]
        __cause: Arc<AtomicError>,
    },
}

/// What a policy answers in place of sending its call, or of waiting for it
/// any longer: one member per mechanism that refuses, the next one added with
/// the mechanism. [`ResilienceError::Refused`] carries it, and so does
/// [`ResilienceEvent::Refusal`](crate::ResilienceEvent::Refusal), but for a
/// timeout, which [`ResilienceEvent::Timeout`](crate::ResilienceEvent::Timeout)
/// tells as the attempt it gave up, the run going on or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResilienceRefusal {
    /// The last attempt did not answer within the policy's timeout, or the
    /// unit's deadline came first.
    #[non_exhaustive]
    TimedOut {
        /// What the attempt was given up at: the policy's timeout — the one a
        /// `#[resilient]` method pins, for its own runs — or what was left of
        /// the unit's deadline when that was shorter: none, for a call the unit
        /// had no time left to send.
        timeout: Duration,
    },
    /// The circuit breaker is open, or another call holds its probe's turn:
    /// the call was refused without being sent.
    #[non_exhaustive]
    Open {
        /// How long until the breaker lets a call through again, at the
        /// earliest — or, while another call probes the dependency, until that
        /// probe has answered at the latest, and that probe's whole bound once
        /// its answer is a response its caller holds, which ends when that
        /// caller has read or dropped it: the `Retry-After` every edge
        /// answers, in whole seconds rounded up.
        retry_after: Duration,
    },
    /// The bulkhead took the call into no slot, for one of two causes the
    /// refusal does not tell apart: every slot stayed taken for as long as the
    /// call waits for one — at once, with no wait — or every place to wait
    /// was taken, `CONCURRENCY_WAITERS` calls waiting already, which the
    /// `waiters` of the refusal's line alone says. It carries no count: the
    /// calls running and waiting are read on the policy, as they stand at the
    /// read, through
    /// [`concurrency_running`](crate::ResiliencePolicy::concurrency_running)
    /// and
    /// [`concurrency_waiting`](crate::ResiliencePolicy::concurrency_waiting).
    Full,
    /// The rate limit had no token free for the attempt, and the next one due
    /// it, behind the attempts already waiting for theirs, came past what it
    /// could wait or past its unit's deadline: the call was refused without
    /// being sent, and is not attempted again.
    #[non_exhaustive]
    Limited {
        /// How long until that token is free: the `Retry-After` every edge
        /// answers, in whole seconds rounded up.
        retry_after: Duration,
    },
}

impl ResilienceRefusal {
    /// A call its unit had no time left to send: refused unsent, so no
    /// attempt, where any other [`TimedOut`](Self::TimedOut) is one given up
    /// at its bound, which is never zero.
    pub(crate) const OUT_OF_TIME: Self = Self::TimedOut {
        timeout: Duration::ZERO,
    };

    /// The wait the refusal knows before a call is let through, which every
    /// edge answers as its `Retry-After`: an open breaker's and a reached rate
    /// limit's.
    #[must_use]
    pub const fn retry_after(self) -> Option<Duration> {
        match self {
            Self::Open { retry_after } | Self::Limited { retry_after } => Some(retry_after),
            Self::TimedOut { .. } | Self::Full => None,
        }
    }

    /// The word the refusals' counter says.
    #[cfg(feature = "opentelemetry")]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::TimedOut { .. } => "timed_out",
            Self::Open { .. } => "open",
            Self::Full => "full",
            Self::Limited { .. } => "limited",
        }
    }
}

impl fmt::Display for ResilienceRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        said(*self, "", f)
    }
}

/// `refusal`'s sentence, naming the instance of the policy that made it after
/// the mechanism that refused, and none for the default one.
fn said(refusal: ResilienceRefusal, instance: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let of = of_instance(instance);
    match refusal {
        ResilienceRefusal::TimedOut { .. } if instance.is_empty() => f.write_str(TIMED_OUT),
        ResilienceRefusal::TimedOut { .. } => write!(
            f,
            "the call did not answer within the timeout of the instance `{instance}`"
        ),
        ResilienceRefusal::Open { .. } => write!(f, "the circuit breaker{of} is open"),
        ResilienceRefusal::Full => write!(f, "the bulkhead{of} has no free slot"),
        ResilienceRefusal::Limited { .. } => write!(f, "the rate limit{of} is reached"),
    }
}

impl<E> ResilienceError<E> {
    /// The policy's refusal the error is, or `None` for the call's own error
    /// and an unsettled savepoint.
    #[must_use]
    pub const fn refusal(&self) -> Option<ResilienceRefusal> {
        match self {
            Self::Refused { refusal, .. } => Some(*refusal),
            Self::Failed(_) | Self::Unsettled { .. } => None,
        }
    }

    /// The run ended on its attempt's savepoint, which `cause` left unsettled.
    pub(crate) fn unsettled(cause: AtomicError) -> Self {
        Self::Unsettled {
            __cause: Arc::new(cause),
        }
    }

    /// `refusal`, made by the policy of `instance`, as its caller is answered.
    pub(crate) fn refused(refusal: ResilienceRefusal, instance: &'static str) -> Self {
        let problem = match refusal.retry_after() {
            Some(wait) => {
                UNAVAILABLE.with_retry_after(nest_rs_core::__private::retry_after_secs(wait))
            }
            None => UNAVAILABLE,
        };
        Self::Refused {
            refusal,
            instance,
            __problem: Arc::new(problem),
        }
    }
}

impl<E> fmt::Display for ResilienceError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(_) => f.write_str(FAILED),
            Self::Refused {
                refusal, instance, ..
            } => said(*refusal, instance, f),
            Self::Unsettled { .. } => f.write_str("an attempt's savepoint could not be settled"),
        }
    }
}

// By hand: each reader of a chain downcasts a link to the type it reads, and
// thiserror's `#[source]` would hand out the `Arc` holding the problem or the cause.
impl<E: Error + 'static> Error for ResilienceError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Failed(error) => Some(error),
            Self::Refused { __problem, .. } => Some(&**__problem),
            Self::Unsettled { __cause } => Some(&**__cause),
        }
    }
}

/// What a refusal's sentence says of the policy that made it, after the
/// mechanism that refused: the instance it is named for, and nothing for the
/// default one.
fn of_instance(instance: &str) -> impl fmt::Display + '_ {
    fmt::from_fn(move |f| match instance {
        "" => Ok(()),
        named => write!(f, " of the instance `{named}`"),
    })
}

nest_rs_core::inventory::submit! { SafeLink::of(&say) }

/// A refusal no call's error type carries — the `ResilienceError<Infallible>`
/// the middleware answers, and ends a read of the body it covers with, inside
/// reqwest's own error — said as `error_message` says any refusal: its
/// sentence and the problem it answers, constants both. A sayer reading a
/// chain says a cause of another crate's only where that crate claims it:
/// unclaimed, a body the policy gave up reads as one that did not decode.
///
/// An unsettled savepoint is not claimed: its chain is the database's to say.
fn say(refused: &ResilienceError<Infallible>) -> Option<String> {
    match refused {
        ResilienceError::Refused { __problem, .. } => Some(format!("{refused}: {__problem}")),
        ResilienceError::Failed(never) => match *never {},
        ResilienceError::Unsettled { .. } => None,
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

    #[derive(Clone, Debug, thiserror::Error)]
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

    /// `refusal`, as the default instance's policy answers it.
    fn refused<E>(refusal: ResilienceRefusal) -> ResilienceError<E> {
        ResilienceError::refused(refusal, "")
    }

    #[test]
    fn every_refusal_is_a_503_and_an_open_breaker_and_a_reached_rate_say_their_wait_rounded_up() {
        let refusals: [ResilienceError<Upstream>; 4] = [
            refused(ResilienceRefusal::TimedOut {
                timeout: Duration::from_secs(5),
            }),
            refused(ResilienceRefusal::Open {
                retry_after: Duration::from_millis(2_500),
            }),
            refused(ResilienceRefusal::Full),
            refused(ResilienceRefusal::Limited {
                retry_after: Duration::from_millis(40_200),
            }),
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
        let open: ResilienceError<Upstream> = refused(ResilienceRefusal::Open {
            retry_after: Duration::from_secs(30),
        });
        let found = nest_rs_core::__private::find_in_chain::<Problem>(&open)
            .expect("the problem is in the chain");
        assert_eq!(found.retry_after(), Some(30));
        assert_eq!(
            error_message(&open),
            "the circuit breaker is open: 503 UNAVAILABLE"
        );

        let limited: ResilienceError<Upstream> = refused(ResilienceRefusal::Limited {
            retry_after: Duration::from_secs(12),
        });
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

    /// A sayer reading the chain beneath an error of its own asks for a link
    /// another crate claims: the refusal no call's error type carries says
    /// what `error_message` says of any refusal, alone or behind a pointer.
    #[test]
    fn a_refusal_carrying_no_calls_error_is_claimed_and_said_with_its_problem() {
        use nest_rs_core::__private::claimed_link;

        let refusals = [
            (
                ResilienceRefusal::TimedOut {
                    timeout: Duration::from_secs(5),
                },
                "the call did not answer within its timeout: 503 UNAVAILABLE",
            ),
            (
                ResilienceRefusal::Open {
                    retry_after: Duration::from_secs(30),
                },
                "the circuit breaker is open: 503 UNAVAILABLE",
            ),
            (
                ResilienceRefusal::Full,
                "the bulkhead has no free slot: 503 UNAVAILABLE",
            ),
            (
                ResilienceRefusal::Limited {
                    retry_after: Duration::from_secs(12),
                },
                "the rate limit is reached: 503 UNAVAILABLE",
            ),
        ];
        for (refusal, said) in refusals {
            let bare: ResilienceError<Infallible> = refused(refusal);
            assert_eq!(claimed_link(&bare).as_deref(), Some(said));
            assert_eq!(error_message(&bare), said);
            let carried: ResilienceError<Upstream> = refused(refusal);
            assert_eq!(
                error_message(&carried),
                said,
                "as a call's own refusal is said"
            );
            assert_eq!(
                claimed_link(&std::sync::Arc::new(bare)).as_deref(),
                Some(said)
            );
        }
    }

    /// A run's own error is no link the crate claims — its chain is the
    /// call's, and an unsettled savepoint's the database's — so each is said
    /// link by link, by whoever claims one.
    #[test]
    fn a_calls_own_failure_and_an_unsettled_savepoint_are_claimed_by_no_one() {
        use nest_rs_core::__private::claimed_link;

        assert_eq!(claimed_link(&ResilienceError::Failed(Upstream)), None);
        let full: ResilienceError<Upstream> = refused(ResilienceRefusal::Full);
        assert_eq!(claimed_link(&full), None);
        let unsettled: ResilienceError<Infallible> =
            ResilienceError::unsettled(AtomicError::Poisoned { retryable: false });
        assert_eq!(claimed_link(&unsettled), None);
    }

    /// The call's error is read as itself before its chain: a domain error's
    /// own problem is never lost under the policy.
    #[test]
    fn a_failed_call_answers_the_problem_its_error_says() {
        let said = ResilienceError::Failed(NoParcel).to_problem();
        assert_eq!(said, Some(Problem::new(404, code::NOT_FOUND)));
    }

    /// The instance `error`, a refusal, names its policy by.
    fn instance<E>(error: &ResilienceError<E>) -> Option<&'static str> {
        match error {
            ResilienceError::Refused { instance, .. } => Some(instance),
            ResilienceError::Failed(_) | ResilienceError::Unsettled { .. } => None,
        }
    }

    /// Each refusal says the instance of the policy that made it, after the
    /// mechanism that refused, where every reader of its chain reads it, and
    /// the default instance's names none; the problem it answers is the same
    /// whichever policy refused.
    #[test]
    fn a_refusal_names_the_instance_of_its_policy_and_its_problem_names_none() {
        use nest_rs_core::__private::claimed_link;

        let refusals = [
            (
                ResilienceRefusal::TimedOut {
                    timeout: Duration::from_secs(5),
                },
                "the call did not answer within the timeout of the instance `payments`",
                TIMED_OUT,
            ),
            (
                ResilienceRefusal::Open {
                    retry_after: Duration::from_secs(30),
                },
                "the circuit breaker of the instance `payments` is open",
                "the circuit breaker is open",
            ),
            (
                ResilienceRefusal::Full,
                "the bulkhead of the instance `payments` has no free slot",
                "the bulkhead has no free slot",
            ),
            (
                ResilienceRefusal::Limited {
                    retry_after: Duration::from_secs(12),
                },
                "the rate limit of the instance `payments` is reached",
                "the rate limit is reached",
            ),
        ];
        for (refusal, named, unnamed) in refusals {
            let of_payments = ResilienceError::<Upstream>::refused(refusal, "payments");
            let of_the_default = refused::<Upstream>(refusal);
            assert_eq!(instance(&of_payments), Some("payments"));
            assert_eq!(instance(&of_the_default), Some(""));
            assert_eq!(of_payments.to_string(), named);
            assert_eq!(of_the_default.to_string(), unnamed);
            assert_eq!(
                error_message(&of_payments),
                format!("{named}: 503 UNAVAILABLE")
            );
            let bare = ResilienceError::<Infallible>::refused(refusal, "payments");
            assert_eq!(
                claimed_link(&bare),
                Some(format!("{named}: 503 UNAVAILABLE"))
            );
            assert_eq!(
                of_payments.to_problem(),
                of_the_default.to_problem(),
                "{named}: the reply names no policy"
            );
        }
    }

    /// An error clones where the call's error does, every variant as itself,
    /// the clones of a refusal sharing its one problem and those of an
    /// unsettled savepoint its one cause.
    #[test]
    fn an_error_clones_where_the_calls_error_does_sharing_its_problem_or_its_cause() {
        let errors: [ResilienceError<Upstream>; 6] = [
            ResilienceError::Failed(Upstream),
            ResilienceError::refused(
                ResilienceRefusal::TimedOut {
                    timeout: Duration::from_secs(5),
                },
                "payments",
            ),
            ResilienceError::refused(
                ResilienceRefusal::Open {
                    retry_after: Duration::from_secs(30),
                },
                "payments",
            ),
            refused(ResilienceRefusal::Full),
            ResilienceError::refused(
                ResilienceRefusal::Limited {
                    retry_after: Duration::from_secs(12),
                },
                "payments",
            ),
            ResilienceError::unsettled(AtomicError::Poisoned { retryable: true }),
        ];
        for error in &errors {
            let cloned = error.clone();
            assert_eq!(error_message(&cloned), error_message(error));
            assert_eq!(cloned.to_problem(), error.to_problem());
            assert_eq!(instance(&cloned), instance(error));
            assert_eq!(format!("{cloned:?}"), format!("{error:?}"));
        }
        let [_, _, open, .., unsettled] = &errors;
        let (
            ResilienceError::Refused {
                refusal: ResilienceRefusal::Open { .. },
                __problem: problem,
                ..
            },
            ResilienceError::Refused {
                refusal: ResilienceRefusal::Open { .. },
                __problem: cloned,
                ..
            },
        ) = (open, open.clone())
        else {
            panic!("an open breaker's refusal clones as one: {open:?}");
        };
        assert!(Arc::ptr_eq(problem, &cloned), "one problem, shared");
        let (
            ResilienceError::Unsettled { __cause: cause },
            ResilienceError::Unsettled { __cause: cloned },
        ) = (unsettled, unsettled.clone())
        else {
            panic!("an unsettled savepoint clones as one: {unsettled:?}");
        };
        assert!(Arc::ptr_eq(cause, &cloned), "one cause, shared");
    }
}
