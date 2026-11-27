//! [`Attempting`] — the policies whose attempt this thread is polling now, and
//! what each attempt shares with the calls of its policy it makes: the rule
//! stated whole, which the docs copy once (`fundamentals/failures-and-fallbacks`,
//! *A call inside an attempt of its own policy*) and every other page links.
//!
//! **Who.** A run opened while its thread polls an attempt of its own policy —
//! a decorated method or a `run` calling another of that policy, directly or
//! through a call under another policy, a request a client under that policy's
//! middleware sends there, a call a middleware below it makes inside a
//! request's attempt — is a call that attempt makes, of the innermost such
//! attempt. A call on a task the attempt spawned is a call of its own.
//!
//! - **Slot.** The attempt keeps its slot of the bulkhead until it answers and
//!   shares it with one call at a time ([`SharedSlot`]), which gives it back as
//!   it ends, a response's body as that body ends; a call sharing it lends it
//!   on, so a call three runs deep takes no slot either. A call made while it
//!   is shared takes a free slot of its own, or waits within the bulkhead's own
//!   wait for one or for the attempt's to come back, woken as it does, and
//!   with no wait is refused `Full` at once.
//! - **Turn.** A call let through while the attempt's turn of the breaker is
//!   free shares it, a probe's included, and asks the breaker nothing while
//!   the breaker is in the epoch the attempt was let through in, checked again
//!   after a wait for its token; once the breaker opened, was switched by hand
//!   or had an overdue probe's turn taken, the call meets it as any call does.
//!   A call made while another shares the turn meets the breaker on its own:
//!   refused `Open` beside a probe, counting its own outcome while it is
//!   closed. The turn goes back before the slot, so a call that waited for the
//!   attempt's slot takes its turn too.
//! - **Token.** The first call sent goes out on the attempt's token of the
//!   rate, and every later one takes its own. A call refused unsent gives back
//!   the turn ([`SharedPass`]) and spends no token
//!   ([`spend_token`](Attempting::spend_token)).
//! - **Retries.** Inside an attempt its policy may still send again — its run
//!   idempotent with retries left, or a run of the policy around it so — an
//!   idempotent call is sent once, decided as it opens: an attempt that then
//!   ends without being sent again leaves it sent once. Inside one never sent
//!   again — a run once, a last attempt, a request its transport cannot send
//!   again — it keeps its own retries, which nothing around it multiplies.
//! - **Counting.** The attempt counts once in the breaker: a failure when its
//!   own answer fails or a call sharing its turn ended on a failure, the first
//!   one kept, and otherwise as its own answer is judged. A failure the call
//!   is sent again past is [owed](Owed): forgiven when a later attempt answers,
//!   replaced by a later failure, the attempt's should the call end on it — its
//!   retry given up or refused unsent, the call cut. A call sharing the turn
//!   that a policy between cuts once sent has timed out in the attempt
//!   ([`SharedSent`]); one cut before it is sent, waiting for its token, or
//!   dropped by its caller counts nothing; a panic is a failure; an answer
//!   counted in neither direction fails nothing. Each call is sent under its
//!   own timeout.
//! - **Past the attempt.** What a call holds once the attempt it shares with
//!   has answered — a response handed out of a `run`, its body read
//!   afterwards — is shared with nothing: its slot was freed with the
//!   attempt's, and its failure counts nowhere.

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
    /// Whether its token of the rate is still to take: no call sent on it.
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
/// through in `epoch`, its call not sent yet. Dropped so — refused, cut while
/// it waits for its token, its caller gone — it gives the turn back counting
/// nothing, as a [`Pass`](crate::breaker::Pass) of its own does.
#[must_use = "a shared turn is given back as it is dropped"]
pub(crate) struct SharedPass {
    name: NonZeroU64,
    epoch: u64,
}

impl SharedPass {
    /// The epoch of the breaker the attempt was let through in: once the
    /// breaker opened or was switched by hand since, the turn is no longer
    /// one a call is sent on.
    #[inline]
    pub(crate) const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The call sent on the turn: from now on its outcome is the attempt's.
    pub(crate) fn send(self) -> SharedSent {
        let sent = SharedSent {
            name: self.name,
            epoch: self.epoch,
        };
        std::mem::forget(self);
        sent
    }
}

impl Drop for SharedPass {
    fn drop(&mut self) {
        give_back(self.name, self.epoch, None);
    }
}

/// The turn of the breaker of the attempt named `name` a call was sent on,
/// let through in `epoch`, given back as it ends with what it failed on, or as
/// it is dropped: with a timeout inside a [`Cut`], with nothing elsewhere.
#[must_use = "a shared turn is given back as it ends"]
pub(crate) struct SharedSent {
    name: NonZeroU64,
    epoch: u64,
}

impl SharedSent {
    /// The epoch of the breaker the attempt was let through in.
    #[inline]
    pub(crate) const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The call sharing the turn ended, failing on `failed` or not: the
    /// attempt counts that failure as its own.
    pub(crate) fn ended(self, failed: Option<Cow<'static, str>>) {
        let (name, epoch) = (self.name, self.epoch);
        std::mem::forget(self);
        give_back(name, epoch, failed);
    }

    /// The call sharing the turn failed on `failed` in an attempt it sends
    /// again: the turn goes back to share, and the failure is owed to the
    /// attempt, its own only should the call end on it.
    pub(crate) fn owing(self, failed: Cow<'static, str>) -> Owed {
        let (name, epoch) = (self.name, self.epoch);
        std::mem::forget(self);
        give_back(name, epoch, None);
        Owed {
            name,
            failed: Some(failed),
        }
    }
}

impl Drop for SharedSent {
    fn drop(&mut self) {
        // Dropped by the timeout of a policy between, the call has timed out,
        // as an attempt of its own in flight there would have (`Sent`).
        let failed = Cut::encloses().then_some(Cow::Borrowed(TIMED_OUT));
        give_back(self.name, self.epoch, failed);
    }
}

/// The turn of the attempt named `name`, let through in `epoch`, given back
/// to share, the attempt failing on `failed` unless it already failed.
fn give_back(name: NonZeroU64, epoch: u64, failed: Option<Cow<'static, str>>) {
    ATTEMPTING.with(|attempting| {
        attempting.named(name, |lent| {
            lent.turn = Some(epoch);
            if let Some(failed) = failed {
                lent.met().failed.get_or_insert(failed);
            }
        });
    });
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

/// What a call takes of the attempt it is made in as it arrives: that
/// attempt's turn of the breaker, when free, and whether it may be sent on
/// that attempt's token of the rate, which no call sent took yet.
pub(crate) struct Taken {
    pub(crate) turn: Option<SharedPass>,
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

    /// What a call arriving takes of the innermost attempt of `policy` the
    /// thread polls: its turn of the breaker, when no call shares it now —
    /// sent on only while the breaker is in the turn's epoch — and its token
    /// of the rate, when no call took it, [spent](Self::spend_token) only once
    /// the call is sent; a call refused unsent gives the turn back
    /// ([`SharedPass`]).
    pub(crate) fn take(policy: &Mechanisms) -> Taken {
        let pointer: *const Mechanisms = policy;
        ATTEMPTING
            .with(|attempting| {
                attempting.innermost(pointer, |lent| Taken {
                    turn: lent.turn.take().map(|epoch| SharedPass {
                        name: lent.name(),
                        epoch,
                    }),
                    token: lent.token,
                })
            })
            .unwrap_or(Taken::NOTHING)
    }

    /// The token of the innermost attempt of `policy` the thread polls spent
    /// by the call sent now, when it was still to take: a call refused unsent
    /// spends none, and the next call sent takes it.
    #[cold]
    pub(crate) fn spend_token(policy: &Mechanisms) {
        let pointer: *const Mechanisms = policy;
        ATTEMPTING.with(|attempting| {
            attempting.innermost(pointer, |lent| lent.token = false);
        });
    }
}
