//! [`ProcessAnswer`] — a `#[process]` method's error made its attempt's
//! [`JobError`], read by its type where the expansion names it.

use std::error::Error;

use nest_rs_core::{Problem, ToProblem};

use crate::JobError;

/// Whether a failure answering `problem` dead-letters its job at once: a `4xx`
/// is the client's error (RFC 9110 §15.5), which no retry clears — but `408`,
/// whose request the client may repeat (§15.5.9), and `429` (RFC 6585 §4),
/// which asks for a later one. Every other problem retries within the budget.
pub(crate) const fn dead_letters(problem: &Problem) -> bool {
    let status = problem.status();
    status >= 400 && status < 500 && status != 408 && status != 429
}

/// A failure answering `problem`, classified by it.
pub(crate) fn by_problem(
    problem: &Problem,
    source: impl Into<Box<dyn Error + Send + Sync>> + 'static,
) -> JobError {
    if dead_letters(problem) {
        JobError::abort(source)
    } else {
        JobError::retry(source)
    }
}

/// A `#[process]` method's error, made the attempt's [`JobError`] **by its
/// type**, in tiers, each taken only when the one above does not apply:
///
/// 1. A [`ToProblem`] error takes the inherent method: the problem it says
///    decides — a `4xx` other than `408` and `429` dead-letters the job at
///    once, any other retries — and one it withholds retries.
/// 2. Any other error taking a box takes [`ProcessAnswerChain`]: a
///    [`JobError`] keeps its disposition, a [`Problem`] in its chain decides,
///    and anything else retries within the budget.
pub struct ProcessAnswer<E>(pub E);

impl<E: ToProblem> ProcessAnswer<E> {
    /// The attempt's failure, classified by the problem the error says.
    pub fn into_job_error(self) -> JobError {
        match self.0.to_problem() {
            Some(problem) => by_problem(&problem, self.0),
            None => JobError::retry(self.0),
        }
    }
}

/// The error that is not a [`ToProblem`] case of [`ProcessAnswer`] — the second
/// tier.
pub trait ProcessAnswerChain {
    /// The attempt's failure, classified by what the error's chain carries.
    fn into_job_error(self) -> JobError;
}

impl<E> ProcessAnswerChain for ProcessAnswer<E>
where
    E: Into<Box<dyn Error + Send + Sync>> + 'static,
{
    fn into_job_error(self) -> JobError {
        let error = nest_rs_core::boxed_error(self.0);
        let error = match error.downcast::<JobError>() {
            Ok(chosen) => return *chosen,
            Err(error) => error,
        };
        match nest_rs_core::__private::find_in_chain::<Problem>(&*error) {
            Some(problem) if dead_letters(problem) => JobError::abort(error),
            _ => JobError::retry(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::problem::code;

    use super::*;

    #[test]
    fn a_client_error_dead_letters_but_one_inviting_a_repeat() {
        for (status, dead) in [
            (400, true),
            (404, true),
            (408, false),
            (409, true),
            (422, true),
            (429, false),
            (499, true),
            (500, false),
            (503, false),
        ] {
            assert_eq!(
                dead_letters(&Problem::new(status, code::INVALID_ARGUMENT)),
                dead,
                "{status}"
            );
        }
    }
}
