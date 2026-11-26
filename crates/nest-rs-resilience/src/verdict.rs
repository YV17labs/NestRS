//! [`Verdict`] — how an attempt's answer counts — and [`Ended`], what an
//! attempt ended on.

use std::any::Any;
use std::borrow::Cow;
use std::error::Error;
use std::time::Duration;

use nest_rs_core::Problem;

use crate::breaker::Sent;
use crate::error::Refusal;

/// What an attempt whose call unwound failed on; its payload is its unit's to file.
pub(crate) const PANICKED: &str = "the call panicked";

/// How an attempt's answer counts, on two axes: in the breaker — a success,
/// a failure, or neither — and whether the run sends its call again after it,
/// when it may. An `Ok` is a success, and an `Err` a failure or neither: one
/// variant for each pair that can be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// An `Ok`, the run's answer: a success, not sent again.
    Succeeded,
    /// An `Ok` the run sends its call again after: a success, with no error
    /// to tell and no wait asked.
    Again,
    /// The dependency failed, and the run sends its call again, after the
    /// wait it asked when it said one.
    Failed {
        asked: Option<Duration>,
        /// What it failed on, as the lines telling the attempt say it, said
        /// once where the answer is judged: an error's chain as the policy
        /// tells one ([`told`](crate::told::told)).
        error: Cow<'static, str>,
    },
    /// The dependency failed, and the run ends there.
    Final { error: Cow<'static, str> },
    /// An `Err` counting in neither direction, the run's answer: the breaker
    /// holds nothing of it, and the run ends there.
    Ignored,
    /// An `Err` counting in neither direction, which the run sends its call
    /// again after, after the wait it asked when it said one: the breaker
    /// holds nothing of it, and the retry's line tells its `error`.
    IgnoredAgain {
        asked: Option<Duration>,
        error: Cow<'static, str>,
    },
}

impl Verdict {
    /// The attempt `sent` counted in its breaker as the verdict says: a
    /// success, a failure saying what it failed on, or nothing.
    pub(crate) fn settle(&self, sent: Sent<'_>) {
        match self {
            Self::Succeeded | Self::Again => sent.settle(None),
            Self::Failed { error, .. } | Self::Final { error } => sent.settle(Some(error)),
            Self::Ignored | Self::IgnoredAgain { .. } => sent.ignore(),
        }
    }

    /// Whether the answer judged is an `Ok`, which the savepoint its attempt
    /// ran in keeps when the run answers it.
    pub(crate) const fn answered_ok(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Again)
    }
}

/// What one attempt ended on.
#[derive(Debug)]
pub(crate) enum Ended {
    /// Its call answered, judged so: the answer is the call's to hold.
    Judged(Verdict),
    /// The policy refused it unsent, or gave it up at its bound.
    Refused(Refusal),
    /// Its call, or a predicate judging its answer, panicked: a failure in
    /// the breaker, unwinding on once its savepoint is rolled back.
    Unwound(Box<dyn Any + Send>),
}

/// The wait a [`Problem`] in `error`'s chain asks before a retry.
pub(crate) fn asked(error: &(dyn Error + 'static)) -> Option<Duration> {
    nest_rs_core::__private::find_in_chain::<Problem>(error)
        .and_then(Problem::retry_after)
        .map(|seconds| Duration::from_secs(seconds.into()))
}
