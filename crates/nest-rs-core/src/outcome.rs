//! What running a unit, or the rest of it, came to: an [`Outcome`], either
//! [`Settled`] — the proof the unit ran or was answered — or a [`Failure`]
//! carrying the error with its whole chain.

use std::error::Error;
use std::fmt;
use std::sync::Arc;

use crate::problem::Problem;

/// What running a unit, or the rest of it, came to. The reply itself sits in
/// the edge's view of the unit.
pub type Outcome = Result<Settled, Failure>;

/// Proof that the unit ran or was answered.
///
/// Nothing outside the framework builds one: [`Next::run`](crate::Next::run)
/// returns it once the rest of the unit ran, and an edge's view mints one when
/// a layer writes the reply through it. A layer returning neither is a compile
/// error:
///
/// ```compile_fail,E0308
/// use nest_rs_core::{Interceptor, Layer, Next, Outcome, UnitContext, async_trait};
///
/// struct Forgetful;
/// impl Layer for Forgetful {}
///
/// #[async_trait]
/// impl Interceptor for Forgetful {
///     async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
///         Ok(())
///     }
/// }
/// ```
#[must_use = "return it: it proves the unit ran or was answered"]
#[derive(Debug)]
pub struct Settled(());

/// Why a unit failed: the error with its source chain intact, shared so a layer
/// answering success above it cannot lose it, and whether the edge's view
/// already holds the answer.
///
/// A layer's `?` turns any error into one; an `anyhow::Error` or a boxed error,
/// which implement no `Error`, go through [`Failure::new`].
pub struct Failure {
    error: Arc<dyn Error + Send + Sync>,
    answer: Answer,
}

/// What the edge renders for a failure.
#[derive(Clone)]
enum Answer {
    /// Nothing yet: the edge answers it by its one rule for an error.
    Unanswered,
    /// An exception filter's problem, which the edge renders in its standard
    /// form over whatever the view held.
    Problem(Box<Problem>),
    /// The view holds the reply a layer wrote.
    Answered,
}

impl Failure {
    /// A failure carrying `error`, every link of its chain kept as the type it
    /// is — an `anyhow::Error`'s included.
    pub fn new<E>(error: E) -> Self
    where
        E: Into<Box<dyn Error + Send + Sync>> + 'static,
    {
        Self {
            error: Arc::from(crate::boxed_error(error)),
            answer: Answer::Unanswered,
        }
    }

    /// The same failure, answered by the reply a layer wrote through the
    /// edge's view — `reply` is the proof it did. The unit stays failed.
    pub fn answered(self, reply: Settled) -> Self {
        let Settled(()) = reply;
        Self {
            answer: Answer::Answered,
            ..self
        }
    }

    /// Whether the edge's view already holds the answer to this failure.
    pub fn is_answered(&self) -> bool {
        matches!(self.answer, Answer::Answered)
    }

    /// The error the unit failed with.
    pub fn error(&self) -> &(dyn Error + Send + Sync + 'static) {
        &*self.error
    }

    /// The first `E` in the error's chain, the error itself first.
    pub fn find<E: Error + 'static>(&self) -> Option<&E> {
        crate::error_message::__private::find_in_chain(self.error())
    }

    /// The same failure, to be answered with `problem` in the edge's standard
    /// form, whatever its view held.
    pub(crate) fn with_problem(self, problem: Problem) -> Self {
        Self {
            answer: Answer::Problem(Box::new(problem)),
            ..self
        }
    }

    /// A second handle on this failure, sharing its error.
    pub(crate) fn shared(&self) -> Self {
        Self {
            error: Arc::clone(&self.error),
            answer: self.answer.clone(),
        }
    }
}

impl fmt::Debug for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let answer = match &self.answer {
            Answer::Unanswered => "unanswered",
            Answer::Problem(_) => "problem",
            Answer::Answered => "answered",
        };
        f.debug_struct("Failure")
            .field("error", &crate::error_message(self.error()))
            .field("answer", &answer)
            .finish()
    }
}

/// Coherent beside `From<T> for T` because `Failure` implements no `Error`.
impl<E: Error + Send + Sync + 'static> From<E> for Failure {
    fn from(error: E) -> Self {
        Self {
            error: Arc::new(error),
            answer: Answer::Unanswered,
        }
    }
}

/// `outcome` is private: its tier-2 items live here, reached only through the
/// crate's `__private`.
pub(crate) mod __private {
    use super::{Answer, Failure, Settled};
    use crate::problem::Problem;

    /// The proof an edge's view mints when a layer writes the unit's reply
    /// through it.
    pub fn settled() -> Settled {
        Settled(())
    }

    /// The problem an exception filter answered `failure` with, which the edge
    /// renders in its standard form over whatever its view held.
    pub fn caught_problem(failure: &Failure) -> Option<&Problem> {
        match &failure.answer {
            Answer::Problem(problem) => Some(problem),
            Answer::Unanswered | Answer::Answered => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::__private::{caught_problem, settled};
    use super::*;
    use crate::problem::code;

    #[derive(Debug, thiserror::Error)]
    #[error("the ledger refused the write")]
    struct LedgerRefused;

    #[derive(Debug, thiserror::Error)]
    enum OrderError {
        #[error("the order could not be paid")]
        Payment(#[source] LedgerRefused),
    }

    #[test]
    fn a_failure_finds_an_error_anywhere_in_its_chain() {
        let failure = Failure::from(OrderError::Payment(LedgerRefused));
        assert!(failure.find::<OrderError>().is_some(), "itself first");
        assert!(failure.find::<LedgerRefused>().is_some(), "then its source");
        assert!(failure.find::<std::io::Error>().is_none());
    }

    #[test]
    fn an_anyhow_error_keeps_the_error_it_holds() {
        let failure = Failure::new(anyhow::Error::new(LedgerRefused).context("paying order 42"));
        assert!(failure.find::<LedgerRefused>().is_some());
        assert_eq!(
            crate::error_message(failure.error()),
            "paying order 42: the ledger refused the write"
        );
    }

    #[test]
    fn answering_a_failure_keeps_it_failed_and_its_error() {
        let failure = Failure::from(LedgerRefused);
        assert!(!failure.is_answered());
        let answered = failure.answered(settled());
        assert!(answered.is_answered());
        assert!(answered.find::<LedgerRefused>().is_some());
    }

    #[test]
    fn a_problem_supersedes_what_the_view_held() {
        let answered = Failure::from(LedgerRefused).answered(settled());
        let problem = answered.with_problem(Problem::new(409, code::CONFLICT));
        assert!(
            !problem.is_answered(),
            "the edge renders the problem instead"
        );
        assert_eq!(
            caught_problem(&problem).map(Problem::code),
            Some(code::CONFLICT)
        );
        assert!(caught_problem(&Failure::from(LedgerRefused)).is_none());
    }

    #[test]
    fn a_shared_failure_is_the_same_error() {
        let failure = Failure::from(LedgerRefused);
        let shared = failure.shared();
        assert!(std::ptr::addr_eq(failure.error(), shared.error()));
    }

    #[test]
    fn its_debug_says_the_chain_and_never_more() {
        let failure = Failure::from(OrderError::Payment(LedgerRefused));
        assert_eq!(
            format!("{failure:?}"),
            "Failure { error: \"the order could not be paid: the ledger refused the write\", \
             answer: \"unanswered\" }"
        );
    }
}
