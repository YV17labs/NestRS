//! [`Attempting`] — the policies whose attempt this thread is polling now, and
//! what each attempt lends: a run opened inside one of them, directly or
//! through a call of another policy, is a call of that policy made by that
//! attempt. The first such call takes the attempt's slot of the bulkhead, and
//! the first let through its token of the rate and its turn of the breaker,
//! so it counts once, for the attempt; every other meets the policy as any
//! call does.

use std::cell::{Cell, RefCell};

use crate::breaker::LentAttempt;
use crate::bulkhead::HeldSlot;
use crate::mechanisms::Mechanisms;

/// The depth of nested attempts a thread holds with no allocation.
const INLINE: usize = 8;

/// What an attempt was let through with, lent to the first call of its
/// policy it makes that is let through in turn: its attempt in the breaker,
/// when the policy has one, and its token of the rate, when the policy has a
/// rate — which the attempt took, so the call takes none.
pub(crate) struct Turn {
    pub(crate) attempt: Option<LentAttempt>,
}

/// One attempt a thread is polling: its policy, and what it still lends —
/// read only when it lent anything, so an attempt lending nothing pays a
/// pointer and a flag.
struct Entry {
    policy: Cell<*const Mechanisms>,
    lent: Cell<bool>,
    slot: Cell<Option<HeldSlot>>,
    turn: Cell<Option<Turn>>,
}

impl Entry {
    const fn empty() -> Self {
        Self {
            policy: Cell::new(std::ptr::null()),
            lent: Cell::new(false),
            slot: Cell::new(None),
            turn: Cell::new(None),
        }
    }
}

/// The attempts one thread is polling, outermost first: the first [`INLINE`]
/// in place, so a run allocates nothing to say so, and any deeper behind them.
struct Polling {
    inline: [Entry; INLINE],
    deeper: RefCell<Vec<Entry>>,
    len: Cell<usize>,
}

impl Polling {
    #[inline]
    fn push(&self, policy: *const Mechanisms, slot: Option<HeldSlot>, turn: Option<Turn>) -> usize {
        let len = self.len.get();
        let lent = slot.is_some() || turn.is_some();
        match self.inline.get(len) {
            Some(free) => {
                free.policy.set(policy);
                free.lent.set(lent);
                if lent {
                    free.slot.set(slot);
                    free.turn.set(turn);
                }
            }
            None => self.deeper.borrow_mut().push(Entry {
                policy: Cell::new(policy),
                lent: Cell::new(lent),
                slot: Cell::new(slot),
                turn: Cell::new(turn),
            }),
        }
        self.len.set(len + 1);
        len
    }

    fn contains(&self, policy: *const Mechanisms) -> bool {
        let len = self.len.get();
        self.inline[..len.min(INLINE)]
            .iter()
            .any(|held| held.policy.get() == policy)
            || (len > INLINE
                && self
                    .deeper
                    .borrow()
                    .iter()
                    .any(|held| held.policy.get() == policy))
    }

    /// `take` run on the innermost attempt of `policy` the thread polls.
    fn innermost<T>(&self, policy: *const Mechanisms, take: impl Fn(&Entry) -> T) -> Option<T> {
        let len = self.len.get();
        if len > INLINE
            && let Some(held) = self
                .deeper
                .borrow()
                .iter()
                .rev()
                .find(|held| held.policy.get() == policy)
        {
            return Some(take(held));
        }
        self.inline[..len.min(INLINE)]
            .iter()
            .rev()
            .find(|held| held.policy.get() == policy)
            .map(take)
    }

    /// What the attempt at `at`, the innermost, still lends, taken, and the
    /// attempt let go.
    #[inline]
    fn pop(&self, at: usize) -> (Option<HeldSlot>, Option<Turn>) {
        debug_assert_eq!(self.len.get(), at + 1, "attempts end innermost first");
        self.len.set(at);
        match self.inline.get(at) {
            Some(held) if !held.lent.get() => (None, None),
            Some(held) => (held.slot.take(), held.turn.take()),
            None => self
                .deeper
                .borrow_mut()
                .pop()
                .map_or((None, None), |held| (held.slot.take(), held.turn.take())),
        }
    }
}

thread_local! {
    static ATTEMPTING: Polling = const {
        Polling {
            inline: [const { Entry::empty() }; INLINE],
            deeper: RefCell::new(Vec::new()),
            len: Cell::new(0),
        }
    };
}

/// One poll of an attempt's call under the policy it holds, lending what the
/// attempt holds until [`leave`](Self::leave) hands back what no call took:
/// the run's guard leaves as the poll ends, a panic out of it included.
pub(crate) struct Attempting {
    /// How many attempts the thread was polling already.
    around: usize,
}

impl Attempting {
    /// The poll of an attempt of `policy`'s, lending its `slot` and its `turn`.
    #[inline]
    pub(crate) fn enter(policy: &Mechanisms, slot: Option<HeldSlot>, turn: Option<Turn>) -> Self {
        let around = ATTEMPTING.with(|attempting| attempting.push(policy, slot, turn));
        Self { around }
    }

    /// The poll over, what the attempt lent and no call took handed back.
    #[inline]
    pub(crate) fn leave(&self) -> (Option<HeldSlot>, Option<Turn>) {
        ATTEMPTING.with(|attempting| attempting.pop(self.around))
    }

    /// Whether this thread is polling an attempt of `policy`'s now.
    #[inline]
    pub(crate) fn inside(policy: &Mechanisms) -> bool {
        let pointer: *const Mechanisms = policy;
        ATTEMPTING.with(|attempting| attempting.contains(pointer))
    }

    /// The slot the innermost attempt of `policy` the thread polls still
    /// lends, taken.
    pub(crate) fn slot(policy: &Mechanisms) -> Option<HeldSlot> {
        let pointer: *const Mechanisms = policy;
        ATTEMPTING
            .with(|attempting| attempting.innermost(pointer, |held| held.slot.take()))
            .flatten()
    }

    /// The turn the innermost attempt of `policy` the thread polls still
    /// lends, taken.
    pub(crate) fn turn(policy: &Mechanisms) -> Option<Turn> {
        let pointer: *const Mechanisms = policy;
        ATTEMPTING
            .with(|attempting| attempting.innermost(pointer, |held| held.turn.take()))
            .flatten()
    }
}
