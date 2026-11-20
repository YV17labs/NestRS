//! [`ResilienceError`] — a call through a policy that did not answer `Ok`.

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{Problem, ToProblem};

use crate::code;

/// A call through a [`Policy`](crate::Policy) that did not answer `Ok`: one of
/// the policy's own refusals, or the call's own error after its last attempt.
///
/// It implements [`ToProblem`] whenever `E` does: a refusal answers its own
/// code ([`code::TIMEOUT`], [`code::CIRCUIT_OPEN`], [`code::BULKHEAD_FULL`]),
/// and the call's own error answers what that error says.
#[derive(Debug)]
#[non_exhaustive]
pub enum ResilienceError<E> {
    /// An attempt outlived its timeout, or the call its deadline, on the last
    /// attempt the call was allowed. The attempt's cancellation token was
    /// cancelled and its future dropped.
    Timeout {
        /// The policy the call went through.
        policy: Arc<str>,
        /// How long the last attempt was given.
        after: Duration,
        /// How many attempts were made.
        attempts: u32,
    },
    /// The breaker was open: the call was not made (or not made again).
    CircuitOpen {
        /// The policy the call went through.
        policy: Arc<str>,
        /// About how long until the breaker lets a probe through.
        retry_after: Duration,
    },
    /// Every slot of the bulkhead was taken for as long as the call could wait
    /// for one: the call was not made.
    BulkheadFull {
        /// The policy the call went through.
        policy: Arc<str>,
    },
    /// The call's own error, from its last attempt.
    Failed {
        /// The policy the call went through.
        policy: Arc<str>,
        /// How many attempts were made.
        attempts: u32,
        /// What the last attempt answered.
        error: E,
    },
}

impl<E> ResilienceError<E> {
    /// The name of the policy the call went through.
    pub fn policy(&self) -> &str {
        match self {
            Self::Timeout { policy, .. }
            | Self::CircuitOpen { policy, .. }
            | Self::BulkheadFull { policy }
            | Self::Failed { policy, .. } => policy,
        }
    }

    /// Whether the policy refused the call before making it: an open breaker
    /// or a full bulkhead. A fallback may answer then without the risk of a
    /// duplicate side effect.
    pub const fn is_rejected(&self) -> bool {
        matches!(self, Self::CircuitOpen { .. } | Self::BulkheadFull { .. })
    }

    /// The call's own error, when that is how it ended.
    pub fn into_inner(self) -> Option<E> {
        match self {
            Self::Failed { error, .. } => Some(error),
            _ => None,
        }
    }

    /// The call's own error, borrowed, when that is how it ended.
    pub const fn inner(&self) -> Option<&E> {
        match self {
            Self::Failed { error, .. } => Some(error),
            _ => None,
        }
    }

    /// The problem the policy's own refusal answers; `None` for the call's own
    /// error, which says its own.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use nest_rs_resilience::{ResilienceError, code};
    ///
    /// let open: ResilienceError<std::io::Error> = ResilienceError::CircuitOpen {
    ///     policy: Arc::from("payments"),
    ///     retry_after: Duration::from_millis(2500),
    /// };
    /// let problem = open.problem().expect("a refusal");
    /// assert_eq!((problem.status(), problem.code()), (503, code::CIRCUIT_OPEN));
    /// assert_eq!(problem.retry_after(), Some(3));
    /// ```
    pub fn problem(&self) -> Option<Problem> {
        match self {
            Self::Timeout { .. } => Some(Problem::new(504, code::TIMEOUT)),
            Self::CircuitOpen { retry_after, .. } => {
                Some(Problem::new(503, code::CIRCUIT_OPEN).with_retry_after(seconds(*retry_after)))
            }
            Self::BulkheadFull { .. } => Some(Problem::new(503, code::BULKHEAD_FULL)),
            Self::Failed { .. } => None,
        }
    }

    /// Maps the call's own error, leaving a refusal as it is.
    pub fn map_inner<F>(self, map: impl FnOnce(E) -> F) -> ResilienceError<F> {
        match self {
            Self::Timeout {
                policy,
                after,
                attempts,
            } => ResilienceError::Timeout {
                policy,
                after,
                attempts,
            },
            Self::CircuitOpen {
                policy,
                retry_after,
            } => ResilienceError::CircuitOpen {
                policy,
                retry_after,
            },
            Self::BulkheadFull { policy } => ResilienceError::BulkheadFull { policy },
            Self::Failed {
                policy,
                attempts,
                error,
            } => ResilienceError::Failed {
                policy,
                attempts,
                error: map(error),
            },
        }
    }
}

/// `wait` in whole seconds, rounded up and at least one: a `Retry-After` of
/// zero tells a client to come back at once, into the same open breaker.
fn seconds(wait: Duration) -> u32 {
    let whole = wait
        .as_secs()
        .saturating_add(u64::from(wait.subsec_nanos() > 0));
    u32::try_from(whole).unwrap_or(u32::MAX).max(1)
}

impl<E: fmt::Display> fmt::Display for ResilienceError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout {
                policy,
                after,
                attempts,
            } => write!(
                f,
                "the call through policy `{policy}` timed out after {after:?} (attempt {attempts})"
            ),
            Self::CircuitOpen {
                policy,
                retry_after,
            } => write!(
                f,
                "the circuit of policy `{policy}` is open; retry in about {retry_after:?}"
            ),
            Self::BulkheadFull { policy } => {
                write!(f, "the bulkhead of policy `{policy}` is full")
            }
            Self::Failed {
                policy, attempts, ..
            } => write!(
                f,
                "the call through policy `{policy}` failed after {attempts} attempt(s)"
            ),
        }
    }
}

impl<E: Error + 'static> Error for ResilienceError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Failed { error, .. } => Some(error),
            _ => None,
        }
    }
}

impl<E: ToProblem> ToProblem for ResilienceError<E> {
    fn to_problem(&self) -> Option<Problem> {
        match self {
            Self::Failed { error, .. } => error.to_problem(),
            _ => self.problem(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed() -> ResilienceError<std::io::Error> {
        ResilienceError::Failed {
            policy: Arc::from("carrier"),
            attempts: 3,
            error: std::io::Error::other("connection reset"),
        }
    }

    #[test]
    fn retry_after_rounds_up_and_is_never_zero() {
        assert_eq!(seconds(Duration::ZERO), 1);
        assert_eq!(seconds(Duration::from_millis(1)), 1);
        assert_eq!(seconds(Duration::from_secs(2)), 2);
        assert_eq!(seconds(Duration::from_millis(2001)), 3);
        assert_eq!(seconds(Duration::MAX), u32::MAX);
    }

    #[test]
    fn each_refusal_answers_its_code() {
        let policy: Arc<str> = Arc::from("p");
        let timeout: ResilienceError<std::io::Error> = ResilienceError::Timeout {
            policy: policy.clone(),
            after: Duration::from_secs(1),
            attempts: 1,
        };
        let full: ResilienceError<std::io::Error> = ResilienceError::BulkheadFull { policy };
        let timeout = timeout.problem().unwrap();
        assert_eq!((timeout.status(), timeout.code()), (504, code::TIMEOUT));
        let full = full.problem().unwrap();
        assert_eq!((full.status(), full.code()), (503, code::BULKHEAD_FULL));
        assert!(failed().problem().is_none());
    }

    #[test]
    fn the_calls_own_error_is_the_source() {
        let error = failed();
        assert_eq!(error.policy(), "carrier");
        assert!(!error.is_rejected());
        assert_eq!(
            nest_rs_core::error_message(&error),
            "the call through policy `carrier` failed after 3 attempt(s): connection reset"
        );
        assert_eq!(error.into_inner().unwrap().to_string(), "connection reset");
    }

    #[test]
    fn the_calls_own_problem_is_answered_through() {
        let inner = Problem::new(409, nest_rs_core::code::CONFLICT);
        let error = ResilienceError::Failed {
            policy: Arc::from("p"),
            attempts: 1,
            error: inner.clone(),
        };
        assert_eq!(error.to_problem(), Some(inner));
        let mapped = error.map_inner(|p| p.status());
        assert_eq!(mapped.inner(), Some(&409));
    }
}
