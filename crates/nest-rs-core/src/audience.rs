//! Who a value being written answers, what refused to be written for them, and
//! whether what is being built is a push: a wire row judges itself, and checks
//! whom it answers, in calls that run on one thread from start to end — a
//! serializer, a stream's poll.

use std::cell::Cell;
use std::fmt;
use std::thread::LocalKey;

use crate::handler::Handler;
use crate::problem::{Problem, code};
use crate::served_address::ServedAddress;

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

impl AnswerRefusal {
    /// What the unit answers in the value's place: `FORBIDDEN` for a denial, the
    /// opaque `INTERNAL` for a defect.
    pub const fn problem(self) -> Problem {
        match self {
            Self::Withheld => Problem::new(403, code::FORBIDDEN),
            Self::Unjudged => Problem::new(500, code::INTERNAL),
        }
    }
}

/// A value's refusal to be written, as the edge answering with it hands it on: its
/// line, filed once that edge names the unit it answers.
#[derive(Clone, Copy)]
pub struct RefusedAnswer {
    refusal: AnswerRefusal,
    entity: &'static str,
    reason: &'static str,
    file: FileRefusal,
}

/// How a refusal files its line, at the unit an edge names.
pub type FileRefusal = fn(&RefusedAnswer, Option<&Handler>, ServedAddress<'_>);

impl RefusedAnswer {
    /// A refusal of a row of `entity`, for `reason`, whose line `file` writes.
    pub const fn new(
        refusal: AnswerRefusal,
        entity: &'static str,
        reason: &'static str,
        file: FileRefusal,
    ) -> Self {
        Self {
            refusal,
            entity,
            reason,
            file,
        }
    }

    /// Why the value refused.
    pub fn refusal(&self) -> AnswerRefusal {
        self.refusal
    }

    /// The table of the row that refused.
    pub fn entity(&self) -> &'static str {
        self.entity
    }

    /// The value an incident query groups on.
    pub fn reason(&self) -> &'static str {
        self.reason
    }

    /// File the refusal's line at the unit `handler` dispatches, served at
    /// `served`; `None` and the default where no unit is answering.
    pub fn file_at(&self, handler: Option<&Handler>, served: ServedAddress<'_>) {
        (self.file)(self, handler, served);
    }
}

impl fmt::Debug for RefusedAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RefusedAnswer")
            .field("refusal", &self.refusal)
            .field("entity", &self.entity)
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// A wire row `#[expose]` builds, as an edge asks whether it answers the caller
/// being answered: implemented by the decorator on its wire type, never by hand.
pub trait WireRow {
    /// Why the row refuses that caller, `None` when it answers.
    fn refusal(&self) -> Option<RefusedAnswer>;
}

thread_local! {
    static AUDIENCE: Cell<Audience> = const { Cell::new(Audience::Caller) };
    /// `None` outside [`answering`]; inside, the first refusal its writer met.
    static CLAIMED: Cell<Option<Option<RefusedAnswer>>> = const { Cell::new(None) };
    static PUSHED: Cell<bool> = const { Cell::new(false) };
}

/// Run `run` with `cell` holding `value`, and what it held before after it, an
/// unwind included.
fn holding<C: Copy + 'static, T>(
    cell: &'static LocalKey<Cell<C>>,
    value: C,
    run: impl FnOnce() -> T,
) -> T {
    struct Restore<C: Copy + 'static>(&'static LocalKey<Cell<C>>, C);
    impl<C: Copy + 'static> Drop for Restore<C> {
        fn drop(&mut self) {
            self.0.with(|cell| cell.set(self.1));
        }
    }
    let _restore = Restore(cell, cell.with(|cell| cell.replace(value)));
    run()
}

/// Run `build`, every value it builds a push, which no read under the caller's
/// ability filtered: a subscription's items, built as its stream is polled.
pub fn as_pushed<T>(build: impl FnOnce() -> T) -> T {
    holding(&PUSHED, true, build)
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
    holding(&AUDIENCE, Audience::Others, write)
}

/// Run `write`, the serializer an edge answers with: a refusal it meets is handed
/// back for the edge to file at the unit it answers, rather than filed here.
pub fn answering<T>(write: impl FnOnce() -> T) -> (T, Option<RefusedAnswer>) {
    holding(&CLAIMED, Some(None), || {
        let written = write();
        (written, CLAIMED.with(Cell::get).flatten())
    })
}

/// A value refused to be written: handed to the edge whose serializer met it
/// ([`answering`]), else filed at once, naming no unit.
pub fn refuse_answer(refused: RefusedAnswer) {
    let unclaimed = CLAIMED.with(|claimed| match claimed.get() {
        Some(None) => {
            claimed.set(Some(Some(refused)));
            false
        }
        Some(Some(_)) => false,
        None => true,
    });
    if unclaimed {
        refused.file_at(None, ServedAddress::default());
    }
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

    use std::sync::atomic::{AtomicUsize, Ordering};

    static FILED: AtomicUsize = AtomicUsize::new(0);

    fn counted(_: &RefusedAnswer, _: Option<&Handler>, _: ServedAddress<'_>) {
        FILED.fetch_add(1, Ordering::SeqCst);
    }

    const WITHHELD: RefusedAnswer = RefusedAnswer::new(
        AnswerRefusal::Withheld,
        "widgets",
        "row_not_granted",
        counted,
    );

    #[test]
    fn a_refusal_an_edge_answers_with_is_handed_back_and_any_other_filed() {
        let before = FILED.load(Ordering::SeqCst);
        let ((), refused) = answering(|| refuse_answer(WITHHELD));
        assert_eq!(
            refused.map(|refused| refused.reason()),
            Some("row_not_granted")
        );
        assert_eq!(FILED.load(Ordering::SeqCst), before, "the edge files it");

        refuse_answer(WITHHELD);
        assert_eq!(
            FILED.load(Ordering::SeqCst),
            before + 1,
            "no edge claimed it"
        );
        let ((), refused) = answering(|| ());
        assert!(refused.is_none(), "a refusal outside the writer is not its");
    }

    #[test]
    fn a_writer_that_unwinds_claims_nothing_after_it() {
        let unwound = std::panic::catch_unwind(|| answering(|| panic!("the writer unwinds")));
        assert!(unwound.is_err());
        let before = FILED.load(Ordering::SeqCst);
        refuse_answer(WITHHELD);
        assert_eq!(FILED.load(Ordering::SeqCst), before + 1);
    }
}
