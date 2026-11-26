//! [`Verdict`] — how an attempt's answer counts — and [`Ended`], what an
//! attempt ended on.

use std::error::Error;
use std::time::Duration;

use nest_rs_core::Problem;

use crate::breaker::Unwound;
use crate::error::ResilienceRefusal;

/// How an attempt's answer counts, on two axes: in the breaker — a success,
/// a failure, or neither — and whether the run sends its call again after it,
/// when it may. An `Ok` is a success, and an `Err` a failure or neither: one
/// variant for each pair that can be. What an `Err` failed on stays the
/// call's, put in words only when something tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// An `Ok`, the run's answer: a success, not sent again.
    Succeeded,
    /// An `Ok` the run sends its call again after: a success, with no error
    /// to tell and no wait asked.
    Again,
    /// The dependency failed, and the run sends its call again, after the
    /// wait it asked when it said one.
    Failed { asked: Option<Duration> },
    /// The dependency failed, and the run ends there.
    Final,
    /// An `Err` counting in neither direction, the run's answer: the breaker
    /// holds nothing of it, and the run ends there.
    Ignored,
    /// An `Err` counting in neither direction, which the run sends its call
    /// again after, after the wait it asked when it said one: the breaker
    /// holds nothing of it, and the retry's line tells its error.
    IgnoredAgain { asked: Option<Duration> },
}

impl Verdict {
    /// Whether the answer judged is an `Ok`, which the savepoint its attempt
    /// ran in keeps when the run answers it.
    pub(crate) const fn answered_ok(self) -> bool {
        matches!(self, Self::Succeeded | Self::Again)
    }
}

/// What one attempt ended on.
#[derive(Debug)]
pub(crate) enum Ended {
    /// Its call answered, judged so: the answer is the call's to hold.
    Judged(Verdict),
    /// The policy refused it unsent, or gave it up at its bound.
    Refused(ResilienceRefusal),
    /// Its call, a predicate judging its answer or the words saying its
    /// error panicked: a failure in the breaker, unwinding on once its
    /// savepoint is rolled back.
    Unwound(Unwound),
}

/// The wait a [`Problem`] in `error`'s chain asks before a retry.
pub(crate) fn asked(error: &(dyn Error + 'static)) -> Option<Duration> {
    nest_rs_core::__private::find_in_chain::<Problem>(error)
        .and_then(Problem::retry_after)
        .map(|seconds| Duration::from_secs(seconds.into()))
}
