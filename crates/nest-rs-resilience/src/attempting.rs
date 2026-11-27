//! [`Attempting`] — the policies whose attempt this thread is polling now, and
//! what each attempt shares: a run opened inside one of them, directly or
//! through a call of another policy, is a call of that policy made by that
//! attempt. The attempt keeps its slot of the bulkhead and its turn of the
//! breaker until it answers, and shares each with one such call at a time,
//! which gives it back as it ends; the first call let through takes its token
//! of the rate. A call sharing its turn counts in the attempt's outcome, once,
//! as it ends — failed only when it ends on a failure, not on one it sends
//! again past — and shares it only while the breaker is in the epoch the
//! attempt was let through in. A call made in an attempt its policy may still
//! send again is sent once, decided as the call opens: an attempt that then
//! ends without being sent again leaves it sent once.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::Waker;

use crate::breaker::Cut;
use crate::error::TIMED_OUT;
use crate::mechanisms::Mechanisms;

/// The depth of nested attempts a thread holds with no allocation.
const INLINE: usize = 8;

/// The next attempt to be named, once a call shares anything of it: unique
/// across threads, so what a call gives back on any of them finds its own
/// attempt or none.
static NAMED: AtomicU64 = AtomicU64::new(1);

/// What an attempt shares with the calls of its policy it makes, kept by its
/// run across its polls and lent to the thread for each.
#[derive(Default)]
pub(crate) struct Lent {
    /// The attempt's name, drawn once a call shares anything of it.
    name: Option<NonZeroU64>,
    /// Whether its slot of the bulkhead is free to share: it holds one, and
    /// no call shares it now.
    slot: bool,
    /// The epoch of the breaker its turn was let through in, while that turn
    /// is free to share.
    turn: Option<u64>,
    /// Whether its token of the rate is still to take.
    token: bool,
    /// What only a failure or a wait for its slot brings, out of line so a
    /// run reserves no room for it.
    met: Option<Box<Met>>,
}

/// What the calls an attempt shares with met: a failure, or a wait for its
/// slot.
#[derive(Default)]
struct Met {
    /// What the first call sharing its turn that failed failed on.
    failed: Option<Cow<'static, str>>,
    /// The calls it makes waiting for a slot, woken as its own comes back.
    waiters: Vec<Waker>,
}

impl Lent {
    /// What an attempt sent now shares, holding a slot or not, a turn of the
    /// breaker let through in an epoch or not, and a token of the rate or not:
    /// nothing of the attempt before it, whose calls are over.
    #[inline]
    pub(crate) fn renew(&mut self, slot: bool, turn: Option<u64>, token: bool) {
        self.name = None;
        self.slot = slot;
        self.turn = turn;
        self.token = token;
        if self.met.is_some() {
            self.met = None;
        }
    }

    /// Whether anything is shared or out: an attempt sharing nothing lends the
    /// thread nothing.
    #[inline]
    fn lends(&self) -> bool {
        self.slot || self.turn.is_some() || self.token || self.name.is_some()
    }

    /// What a call sharing the attempt's turn failed on, taken: the attempt
    /// counts it as its own failure.
    #[inline]
    pub(crate) fn failed(&mut self) -> Option<Cow<'static, str>> {
        self.met.as_mut().and_then(|met| met.failed.take())
    }

    /// What the calls the attempt shares with met, made room for.
    fn met(&mut self) -> &mut Met {
        self.met.get_or_insert_with(Box::default)
    }

    /// The attempt's name, drawn now if it has none.
    fn name(&mut self) -> NonZeroU64 {
        *self.name.get_or_insert_with(|| {
            NonZeroU64::new(NAMED.fetch_add(1, Ordering::Relaxed)).unwrap_or(NonZeroU64::MIN)
        })
    }
}

/// The slot of the attempt named `0` a call shares, given back as it is
/// dropped.
#[must_use = "a shared slot is given back as it is dropped"]
pub(crate) struct SharedSlot(NonZeroU64);

impl Drop for SharedSlot {
    fn drop(&mut self) {
        let waiters = ATTEMPTING.with(|attempting| {
            attempting.named(self.0, |lent| {
                lent.slot = true;
                lent.met
                    .as_mut()
                    .map(|met| std::mem::take(&mut met.waiters))
            })
        });
        waiters
            .flatten()
            .into_iter()
            .flatten()
            .for_each(Waker::wake);
    }
}

/// The turn of the breaker of the attempt named `name` a call shares, let
/// through in `epoch`, given back as it ends with what it failed on, or as it
/// is dropped: with a timeout inside a [`Cut`], with nothing elsewhere.
#[must_use = "a shared turn is given back as it ends"]
pub(crate) struct SharedTurn {
    name: NonZeroU64,
    epoch: u64,
}

impl SharedTurn {
    /// The epoch of the breaker the attempt was let through in: once the
    /// breaker opened or was switched by hand since, the turn is no longer
    /// one a call is sent on.
    #[inline]
    pub(crate) const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The call sharing the turn ended, failing on `failed` or not: the
    /// attempt counts that failure as its own.
    pub(crate) fn ended(self, failed: Option<Cow<'static, str>>) {
        let (name, epoch) = (self.name, self.epoch);
        std::mem::forget(self);
        ATTEMPTING.with(|attempting| {
            attempting.named(name, |lent| {
                lent.turn = Some(epoch);
                if let Some(failed) = failed {
                    lent.met().failed.get_or_insert(failed);
                }
            });
        });
    }

    /// The call sharing the turn failed on `failed` in an attempt it sends
    /// again: the turn goes back to share, and the failure is owed to the
    /// attempt, its own only should the call end on it.
    pub(crate) fn owing(self, failed: Cow<'static, str>) -> Owed {
        let name = self.name;
        drop(self);
        Owed {
            name,
            failed: Some(failed),
        }
    }
}

impl Drop for SharedTurn {
    fn drop(&mut self) {
        let epoch = self.epoch;
        // Dropped by the timeout of a policy between, the call has timed out,
        // as an attempt of its own in flight there would have (`Sent`).
        let cut = Cut::encloses();
        ATTEMPTING.with(|attempting| {
            attempting.named(self.name, |lent| {
                lent.turn = Some(epoch);
                if cut {
                    lent.met().failed.get_or_insert(Cow::Borrowed(TIMED_OUT));
                }
            })
        });
    }
}

/// What a call sharing the turn of the attempt named `name` failed on in an
/// attempt it sends again: the attempt's failure once the call is over,
/// dropped with it, unless a later attempt of the call answered and
/// [forgave](Self::forgive) it.
#[must_use = "an owed failure is counted as it is dropped"]
pub(crate) struct Owed {
    name: NonZeroU64,
    failed: Option<Cow<'static, str>>,
}

impl Owed {
    /// The call answered on a later attempt, or failed on one counted in its
    /// place: the attempt it is made in owes nothing to this one.
    pub(crate) fn forgive(mut self) {
        self.failed = None;
    }
}

impl Drop for Owed {
    fn drop(&mut self) {
        if let Some(failed) = self.failed.take() {
            ATTEMPTING.with(|attempting| {
                attempting.named(self.name, |lent| {
                    lent.met().failed.get_or_insert(failed);
                })
            });
        }
    }
}

/// What a call takes of the attempt it is made in as it is let through: that
/// attempt's turn of the breaker, when free, and its token of the rate, when
/// no call took it yet.
pub(crate) struct Taken {
    pub(crate) turn: Option<SharedTurn>,
    pub(crate) token: bool,
}

impl Taken {
    /// What a call made in no attempt of its policy takes: nothing.
    pub(crate) const NOTHING: Self = Self {
        turn: None,
        token: false,
    };
}

/// One attempt a thread is polling: its policy, whether it may still be sent
/// again, and what it lends — read only when it lends anything, so an attempt
/// lending nothing pays a pointer and two flags.
struct Entry {
    policy: Cell<*const Mechanisms>,
    again: Cell<bool>,
    lends: Cell<bool>,
    lent: Cell<Option<Lent>>,
}

impl Entry {
    const fn empty() -> Self {
        Self {
            policy: Cell::new(std::ptr::null()),
            again: Cell::new(false),
            lends: Cell::new(false),
            lent: Cell::new(None),
        }
    }

    /// `change` made to what the entry lends, when it lends anything.
    fn change<T>(&self, change: impl FnOnce(&mut Lent) -> T) -> Option<T> {
        if !self.lends.get() {
            return None;
        }
        let mut lent = self.lent.take()?;
        let changed = change(&mut lent);
        self.lent.set(Some(lent));
        Some(changed)
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
    fn push(&self, policy: *const Mechanisms, again: bool, lent: Option<Lent>) -> usize {
        let len = self.len.get();
        let lends = lent.is_some();
        match self.inline.get(len) {
            Some(free) => {
                free.policy.set(policy);
                free.again.set(again);
                free.lends.set(lends);
                if lends {
                    free.lent.set(lent);
                }
            }
            None => self.deeper.borrow_mut().push(Entry {
                policy: Cell::new(policy),
                again: Cell::new(again),
                lends: Cell::new(lends),
                lent: Cell::new(lent),
            }),
        }
        self.len.set(len + 1);
        len
    }

    /// Whether the innermost attempt of `policy` the thread polls may still
    /// be sent again, when it polls one.
    fn again(&self, policy: *const Mechanisms) -> Option<bool> {
        let len = self.len.get();
        if len > INLINE
            && let Some(again) = self
                .deeper
                .borrow()
                .iter()
                .rev()
                .find(|held| held.policy.get() == policy)
                .map(|held| held.again.get())
        {
            return Some(again);
        }
        self.inline[..len.min(INLINE)]
            .iter()
            .rev()
            .find(|held| held.policy.get() == policy)
            .map(|held| held.again.get())
    }

    /// `change` made to what the innermost attempt of `policy` the thread
    /// polls lends, when it lends anything.
    fn innermost<T>(
        &self,
        policy: *const Mechanisms,
        change: impl FnOnce(&mut Lent) -> T,
    ) -> Option<T> {
        let len = self.len.get();
        if len > INLINE
            && let Some(held) = self
                .deeper
                .borrow()
                .iter()
                .rev()
                .find(|held| held.policy.get() == policy)
        {
            return held.change(change);
        }
        self.inline[..len.min(INLINE)]
            .iter()
            .rev()
            .find(|held| held.policy.get() == policy)
            .and_then(|held| held.change(change))
    }

    /// `change` made to what the attempt named `name` lends, when the thread
    /// polls it now.
    fn named<T>(&self, name: NonZeroU64, change: impl FnOnce(&mut Lent) -> T) -> Option<T> {
        let len = self.len.get();
        let is = |held: &&Entry| held.change(|lent| lent.name == Some(name)) == Some(true);
        if let Some(held) = self.inline[..len.min(INLINE)].iter().find(is) {
            return held.change(change);
        }
        if len > INLINE
            && let Some(held) = self.deeper.borrow().iter().find(is)
        {
            return held.change(change);
        }
        None
    }

    /// What the attempt at `at`, the innermost, lent, taken back, and the
    /// attempt let go.
    #[inline]
    fn pop(&self, at: usize) -> Option<Lent> {
        debug_assert_eq!(self.len.get(), at + 1, "attempts end innermost first");
        self.len.set(at);
        match self.inline.get(at) {
            Some(held) if !held.lends.get() => None,
            Some(held) => held.lent.take(),
            None => self
                .deeper
                .borrow_mut()
                .pop()
                .and_then(|held| held.lent.take()),
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
/// attempt shares until [`leave`](Self::leave) takes it back: the run's guard
/// leaves as the poll ends, a panic out of it included.
pub(crate) struct Attempting {
    /// How many attempts the thread was polling already.
    around: usize,
}

impl Attempting {
    /// The poll of an attempt of `policy`'s, which its run or one around it
    /// may still send `again`, lending what `lent` shares.
    #[inline]
    pub(crate) fn enter(policy: &Mechanisms, again: bool, lent: &mut Lent) -> Self {
        let lent = lent.lends().then(|| std::mem::take(lent));
        let around = ATTEMPTING.with(|attempting| attempting.push(policy, again, lent));
        Self { around }
    }

    /// The poll over, what the attempt lent taken back, as calls left it.
    #[inline]
    pub(crate) fn leave(&self) -> Option<Lent> {
        ATTEMPTING.with(|attempting| attempting.pop(self.around))
    }

    /// Whether the innermost attempt of `policy`'s this thread polls now may
    /// still be sent again, by its run or by one around it, when it polls
    /// one.
    #[inline]
    pub(crate) fn inside(policy: &Mechanisms) -> Option<bool> {
        let pointer: *const Mechanisms = policy;
        ATTEMPTING.with(|attempting| attempting.again(pointer))
    }

    /// The slot of the innermost attempt of `policy` the thread polls, shared,
    /// when no call shares it now.
    pub(crate) fn slot(policy: &Mechanisms) -> Option<SharedSlot> {
        let pointer: *const Mechanisms = policy;
        ATTEMPTING
            .with(|attempting| {
                attempting.innermost(pointer, |lent| {
                    std::mem::take(&mut lent.slot).then(|| SharedSlot(lent.name()))
                })
            })
            .flatten()
    }

    /// `waker` woken as the slot of the innermost attempt of `policy` the
    /// thread polls comes back, when a call shares it now.
    pub(crate) fn wait_for_slot(policy: &Mechanisms, waker: &Waker) {
        let pointer: *const Mechanisms = policy;
        ATTEMPTING.with(|attempting| {
            attempting.innermost(pointer, |lent| {
                if lent.name.is_some() {
                    let waiters = &mut lent.met().waiters;
                    if !waiters.iter().any(|held| held.will_wake(waker)) {
                        waiters.push(waker.clone());
                    }
                }
            })
        });
    }

    /// What a call let through takes of the innermost attempt of `policy` the
    /// thread polls: its turn of the breaker, when no call shares it now —
    /// sent on only while the breaker is in the turn's epoch — and its token
    /// of the rate, when no call took it.
    pub(crate) fn take(policy: &Mechanisms) -> Taken {
        let pointer: *const Mechanisms = policy;
        ATTEMPTING
            .with(|attempting| {
                attempting.innermost(pointer, |lent| Taken {
                    turn: lent.turn.take().map(|epoch| SharedTurn {
                        name: lent.name(),
                        epoch,
                    }),
                    token: std::mem::take(&mut lent.token),
                })
            })
            .unwrap_or(Taken::NOTHING)
    }
}
