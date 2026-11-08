//! Who a value being written answers, what refused to be written for them, and
//! whether what is being built is a push: a wire row judges itself, and checks
//! whom it answers, in calls that run on one thread from start to end — a
//! serializer, a stream's poll.

use std::cell::Cell;

/// Who the value being written answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Audience {
    /// The caller the running unit answers.
    Caller,
    /// Connections a push reaches, which a row judged for the caller never answers.
    Others,
}

/// Why a value refused to be written for its audience.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnswerRefusal {
    /// A row the caller's grants do not let it read: a denial.
    Withheld,
    /// A row judged for someone else, or by nothing: a wiring defect.
    Unjudged,
}

thread_local! {
    static AUDIENCE: Cell<Audience> = const { Cell::new(Audience::Caller) };
    static REFUSED: Cell<Option<AnswerRefusal>> = const { Cell::new(None) };
    static PUSHED: Cell<bool> = const { Cell::new(false) };
}

/// Run `build`, every value it builds a push, which no read under the caller's
/// ability filtered: a subscription's items, built as its stream is polled.
pub fn as_pushed<T>(build: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            PUSHED.with(|pushed| pushed.set(self.0));
        }
    }
    let _restore = Restore(PUSHED.with(|pushed| pushed.replace(true)));
    build()
}

/// Whether the value being built on this thread is a push ([`as_pushed`]).
pub fn is_pushed() -> bool {
    PUSHED.with(Cell::get)
}

/// Who the value being written on this thread answers.
pub fn audience() -> Audience {
    AUDIENCE.with(Cell::get)
}

/// Run `write`, which serializes a push, for [`Audience::Others`].
pub fn for_others<T>(write: impl FnOnce() -> T) -> T {
    struct Restore(Audience);
    impl Drop for Restore {
        fn drop(&mut self) {
            AUDIENCE.with(|audience| audience.set(self.0));
        }
    }
    let _restore = Restore(AUDIENCE.with(|audience| audience.replace(Audience::Others)));
    write()
}

/// Note why a value refused to be written, for the edge whose serializer failed.
pub fn refuse_answer(why: AnswerRefusal) {
    REFUSED.with(|refused| refused.set(Some(why)));
}

/// What [`refuse_answer`] noted on this thread since the last call, cleared.
pub fn take_refused_answer() -> Option<AnswerRefusal> {
    REFUSED.with(Cell::take)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_answers_others_and_the_caller_again_after_it() {
        assert_eq!(audience(), Audience::Caller);
        assert_eq!(for_others(audience), Audience::Others);
        assert_eq!(audience(), Audience::Caller);
    }

    #[test]
    fn a_push_is_one_for_what_it_builds_alone() {
        assert!(!is_pushed());
        assert!(as_pushed(is_pushed));
        assert!(!is_pushed());
    }

    #[test]
    fn a_noted_refusal_is_read_once() {
        refuse_answer(AnswerRefusal::Withheld);
        assert_eq!(take_refused_answer(), Some(AnswerRefusal::Withheld));
        assert_eq!(take_refused_answer(), None);
    }
}
