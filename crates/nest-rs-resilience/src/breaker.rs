//! The circuit breaker a policy keeps: a count window over its latest
//! attempts, forgetting an outcome past its age when outcomes have one, opened
//! by enough failures in it, probing the dependency one call at a time once
//! its open time has passed, closed by enough probes in a row, and opened or
//! closed by hand. An answer counts only while the breaker has neither opened
//! nor been switched by hand since its attempt was let through. Its state
//! lives in the process, behind one lock, and each holder of the lock
//! publishes its phase and epoch in one word as it lets it go: a call let
//! through closed, a call refused open and one more success of a quiet window
//! read that word alone, and take no lock.

use std::any::Any;
use std::borrow::Cow;
use std::cell::Cell;
use std::collections::VecDeque;
use std::error::Error;
use std::mem::ManuallyDrop;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use self::published::{Holding, Published};
use crate::error::{PANICKED, TIMED_OUT};
use crate::teller::{Teller, Transition};
use crate::told::told;

/// The least a call refused beside a probe is told, one tick of the timer:
/// what is left of a probe's bound once it has passed, the probe's own timer
/// giving it up at any moment. A probe whose answer is held past its run has
/// no timer to give it up, and a call refused beside it is told that probe's
/// whole bound instead ([`Answers`]).
const OVERDUE: Duration = Duration::from_millis(1);

/// A policy's circuit breaker, as the next call meets it:
/// [`ResiliencePolicy::breaker_state`](crate::ResiliencePolicy::breaker_state).
///
/// It says what the breaker does to a call arriving now, where the `warn` each
/// transition files says what it does to the dependency: once the open time
/// has passed the read is [`HalfOpen`](Self::HalfOpen), and the line says
/// `half_open` only when a probe's call is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BreakerState {
    /// The call is sent, and its success or its failure counts in the window
    /// — unless the breaker opens, or is switched by hand, before it answers:
    /// its answer then counts nowhere, in that window or the next.
    Closed,
    /// The call is refused unsent, as long as the open time lasts.
    #[non_exhaustive]
    Open {
        /// What is left of the open time, never zero: the wait a call refused
        /// now is told, as [`ResilienceRefusal::Open`](crate::ResilienceRefusal::Open)
        /// says it.
        retry_after: Duration,
    },
    /// The open time has passed, and the breaker lets one probe through at a
    /// time. The call takes the probe's turn when it is free — the open time
    /// over, or between two probes: of the series
    /// [`breaker_probes`](crate::ResilienceConfig::breaker_probes) asks, or
    /// after one whose answer counted in neither direction — and is refused
    /// while another call holds it, told a wait that is that probe's:
    ///
    /// - while the probe's call is sent, or still waits for its token of the
    ///   rate limit, what is left of its bound, when it has answered at the
    ///   latest — and a millisecond once that has passed, its own timer giving
    ///   it up at any moment, until that bound has passed once more: a probe
    ///   whose run was kept and never polled again, which no timer gives up,
    ///   then hands its turn to the next call, and its late answer counts
    ///   nowhere. The timeout of another policy, the one its caller runs
    ///   under, may give it up sooner: it has failed then as at its own, and
    ///   the breaker is open again;
    /// - once the probe's answer is a response its caller holds
    ///   ([`ResilienceMiddleware`](crate::ResilienceMiddleware)), that probe's
    ///   whole bound, however long it has been held: no timer gives a held
    ///   answer up, the turn comes back when its caller has read or dropped
    ///   it, and its reads wait for the dependency no longer than that bound.
    ///
    /// It carries no time: that bound is the probe's own run's.
    HalfOpen,
}

impl BreakerState {
    /// The word the breaker's status says on the meter.
    #[cfg(feature = "opentelemetry")]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => BreakerPhase::Closed.as_str(),
            Self::Open { .. } => BreakerPhase::Open.as_str(),
            Self::HalfOpen => BreakerPhase::HalfOpen.as_str(),
        }
    }
}

/// The phase a transition of a policy's circuit breaker leaves or enters, as
/// [`ResilienceEvent::Transition`](crate::ResilienceEvent::Transition) says
/// it: what the breaker does to the dependency, in the word the transition's
/// `warn` files as its `state`.
///
/// It is no read of the breaker: [`BreakerState`] says what a call arriving
/// now meets, which is half-open as soon as the open time has passed, where
/// the breaker enters [`HalfOpen`](Self::HalfOpen) only as a probe's call is
/// sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BreakerPhase {
    /// Every call is sent, and its success or its failure counts in the
    /// window, when it answers before the breaker next opens or is switched.
    Closed,
    /// No call is sent: until the open time has passed, then until a call
    /// probes the dependency.
    Open,
    /// One call at a time probes the dependency.
    HalfOpen,
}

impl BreakerPhase {
    /// The word a line and the counter say.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::HalfOpen => "half_open",
        }
    }
}

/// What a policy's circuit breaker counts now, as a call arriving now meets
/// it: [`ResiliencePolicy::breaker_stats`](crate::ResiliencePolicy::breaker_stats).
///
/// The window counts while the breaker is closed and is forgotten as it opens,
/// so an open or half-open breaker reads none, and one closed again counts
/// the attempts let through since: none whose call was sent before it opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BreakerStats {
    /// The outcomes the window holds, each a success or a failure: its latest
    /// attempts, up to
    /// [`breaker_window`](crate::ResilienceConfig::breaker_window), none older
    /// than [`breaker_age`](crate::ResilienceConfig::breaker_age) when
    /// outcomes have an age. An attempt answering an `Err` its method's
    /// `record_if` refuses is neither, and none of them, nor is one let
    /// through before the breaker last opened or was switched by hand.
    pub outcomes: usize,
    /// The failures among them: the breaker opens once a failure brings them
    /// to [`breaker_failures`](crate::ResilienceConfig::breaker_failures).
    pub failures: usize,
}

impl BreakerStats {
    /// The share of the outcomes that failed, from `0.0` to `1.0`, and `0.0`
    /// for a window holding none.
    ///
    /// ```
    /// use nest_rs_resilience::ResiliencePolicy;
    ///
    /// fn degraded(policy: &ResiliencePolicy) -> bool {
    ///     policy
    ///         .breaker_stats()
    ///         .is_some_and(|stats| stats.failure_rate() >= 0.5)
    /// }
    /// ```
    #[must_use]
    pub fn failure_rate(&self) -> f64 {
        if self.outcomes == 0 {
            return 0.0;
        }
        self.failures as f64 / self.outcomes as f64
    }
}

/// Where a breaker is: closed, open until an instant, or half-open, its open
/// time passed and calls probing the dependency one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Closed,
    Open {
        until: Instant,
    },
    /// The open time passed, `passed` probes of the series having succeeded.
    /// The lines read it open until a probe's call is `sent`, the dependency
    /// not called yet; the `probe` out, when one holds the turn, is when it
    /// has answered, and `None` is the turn free for the next call.
    HalfOpen {
        passed: usize,
        sent: bool,
        probe: Option<Answers>,
    },
}

/// When the probe out has answered, as far as its breaker knows: what a call
/// refused beside it is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answers {
    /// By `until` at the latest, where its attempt's timer, armed for
    /// `bound`, gives it up.
    By { until: Instant, bound: Duration },
    /// When its caller has read or dropped the answer it holds past its run,
    /// which no timer bounds: a call refused meanwhile is told the bound the
    /// probe was sent under, the longest its reads wait for the dependency.
    #[cfg(feature = "reqwest")]
    Held(Duration),
}

impl Phase {
    /// The phase a line, the counter and an event say.
    const fn told(self) -> BreakerPhase {
        match self {
            Self::Closed => BreakerPhase::Closed,
            Self::Open { .. } | Self::HalfOpen { sent: false, .. } => BreakerPhase::Open,
            Self::HalfOpen { sent: true, .. } => BreakerPhase::HalfOpen,
        }
    }

    /// What a call meeting the breaker in this phase gets: let through —
    /// sent as the breaker is closed (`None`), or taking the probe's turn, in
    /// the phase that makes, its call sent under `bound` unless it postpones
    /// it — or refused for the wait the breaker knows. A probe whose answer
    /// was due a whole bound of its own ago — a run kept and never polled
    /// again, which no timer of its own gives up — hands its turn on: the
    /// arriving call takes it.
    fn admitting(self, bound: Duration) -> Result<Option<Self>, Duration> {
        let (passed, sent) = match self {
            Self::Closed => return Ok(None),
            Self::Open { until } => match left(until) {
                Some(left) => return Err(left),
                None => (0, false),
            },
            Self::HalfOpen {
                passed,
                sent,
                probe: None,
            } => (passed, sent),
            Self::HalfOpen {
                passed,
                sent,
                probe: Some(Answers::By { until, bound: own }),
            } => {
                let now = Instant::now();
                if now < until + own {
                    return Err(until.saturating_duration_since(now).max(OVERDUE));
                }
                (passed, sent)
            }
            #[cfg(feature = "reqwest")]
            Self::HalfOpen {
                probe: Some(Answers::Held(bound)),
                ..
            } => return Err(bound.max(OVERDUE)),
        };
        Ok(Some(Self::HalfOpen {
            passed,
            sent,
            probe: Some(Answers::By {
                until: Instant::now() + bound,
                bound,
            }),
        }))
    }

    /// The state a call arriving now meets.
    fn state(self) -> BreakerState {
        match self {
            Self::Closed => BreakerState::Closed,
            Self::Open { until } => left(until).map_or(BreakerState::HalfOpen, |retry_after| {
                BreakerState::Open { retry_after }
            }),
            Self::HalfOpen { .. } => BreakerState::HalfOpen,
        }
    }
}

/// What is left of an open time ending at `until`, `None` once it has passed.
fn left(until: Instant) -> Option<Duration> {
    until
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
}

/// The latest attempts' outcomes, oldest first, `true` for a failure — beside
/// the instant each was counted at only when outcomes have an age, so a
/// breaker without one stores no instant and reads no clock to count.
#[derive(Debug)]
enum Outcomes {
    Counted(VecDeque<bool>),
    Aged {
        age: Duration,
        counted: VecDeque<(Instant, bool)>,
    },
}

/// The phase, the window of latest outcomes and how many of them failed, and
/// the epoch: how many times the breaker opened or was switched by hand, each
/// voiding what the attempts admitted before it would change or count — and
/// the transitions it made that are still to tell.
#[derive(Debug)]
struct Held {
    phase: Phase,
    outcomes: Outcomes,
    failed: usize,
    epoch: u64,
    /// The transitions made and not told yet, oldest first.
    untold: VecDeque<Transition>,
    /// Whether a thread is telling them: it took the turn as it queued a
    /// transition and found it free, and gives it up as it finds none left,
    /// each in one hold of the lock, so a transition queued meanwhile is its
    /// to tell.
    telling: bool,
}

impl Held {
    fn forget(&mut self) {
        match &mut self.outcomes {
            Outcomes::Counted(outcomes) => outcomes.clear(),
            Outcomes::Aged { counted, .. } => counted.clear(),
        }
        self.failed = 0;
    }

    /// Count an outcome among the latest `window`, once those older than the
    /// age are forgotten: how many failed. `now` is read only where outcomes
    /// have an age.
    fn count(&mut self, window: usize, failed: bool, now: impl FnOnce() -> Instant) -> usize {
        match &mut self.outcomes {
            Outcomes::Counted(outcomes) => {
                if outcomes.len() == window && outcomes.pop_front() == Some(true) {
                    self.failed -= 1;
                }
                outcomes.push_back(failed);
            }
            Outcomes::Aged { age, counted } => {
                let now = now();
                while let Some(&(at, old)) = counted.front()
                    && now.saturating_duration_since(at) > *age
                {
                    counted.pop_front();
                    self.failed -= usize::from(old);
                }
                if counted.len() == window && counted.pop_front().is_some_and(|(_, old)| old) {
                    self.failed -= 1;
                }
                counted.push_back((now, failed));
            }
        }
        self.failed += usize::from(failed);
        self.failed
    }

    /// How many of the window's outcomes would be failures were one more
    /// failure counted at the instant `now` reads, leaving the window as it
    /// stands: what [`count`](Self::count) would answer. `now` is read only
    /// where outcomes have an age.
    fn failing(&self, window: usize, now: impl FnOnce() -> Instant) -> usize {
        let (kept, aged_failed, oldest_kept) = match &self.outcomes {
            Outcomes::Counted(outcomes) => (outcomes.len(), 0, outcomes.front().copied()),
            Outcomes::Aged { age, counted } => {
                let now = now();
                let aged = counted
                    .iter()
                    .take_while(|(at, _)| now.saturating_duration_since(*at) > *age)
                    .fold((0, 0), |(aged, failed), &(_, old)| {
                        (aged + 1, failed + usize::from(old))
                    });
                (
                    counted.len() - aged.0,
                    aged.1,
                    counted.get(aged.0).map(|&(_, failed)| failed),
                )
            }
        };
        let evicted = kept == window && oldest_kept == Some(true);
        self.failed - aged_failed - usize::from(evicted) + 1
    }

    /// What the window counts at the instant `now` reads, leaving it as it
    /// stands: an outcome past its age counts no more, though it is forgotten
    /// only as the next is counted. `now` is read only where outcomes have an
    /// age.
    fn stats(&self, now: impl FnOnce() -> Instant) -> BreakerStats {
        match &self.outcomes {
            Outcomes::Counted(outcomes) => BreakerStats {
                outcomes: outcomes.len(),
                failures: self.failed,
            },
            Outcomes::Aged { age, counted } => {
                let now = now();
                let (aged, failed) = counted
                    .iter()
                    .take_while(|(at, _)| now.saturating_duration_since(*at) > *age)
                    .fold((0, 0), |(aged, failed), &(_, old)| {
                        (aged + 1, failed + usize::from(old))
                    });
                BreakerStats {
                    outcomes: counted.len() - aged,
                    failures: self.failed - failed,
                }
            }
        }
    }
}

/// A breaker's state behind its lock, and what each holder of the lock
/// publishes of it for a call to read with none.
mod published {
    use std::ops::{Deref, DerefMut};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use parking_lot::{Mutex, MutexGuard};
    use tokio::time::Instant;

    use super::{Held, Outcomes, Phase, left};

    /// A breaker's state, reached under its lock alone, and the board each
    /// holder publishes it on as it lets the lock go: no change of the state
    /// goes unpublished.
    pub(super) struct Published {
        held: Mutex<Held>,
        board: Board,
        /// The instant `open_until` counts from.
        origin: Instant,
        /// The latest attempts the window counts, which a quiet one holds.
        window: usize,
    }

    impl Published {
        pub(super) fn new(held: Held, window: usize) -> Self {
            let word = Word::of(&held, window);
            Self {
                held: Mutex::new(held),
                board: Board::new(word.0),
                origin: Instant::now(),
                window,
            }
        }

        /// The state, under the lock until the guard is dropped, which
        /// publishes its word.
        pub(super) fn lock(&self) -> Holding<'_> {
            Holding {
                published: self,
                held: self.held.lock(),
            }
        }

        /// Whether a thread holds the lock now.
        #[cfg(test)]
        pub(super) fn locked(&self) -> bool {
            self.held.try_lock().is_none()
        }

        /// The word the last holder of the lock published.
        pub(super) fn word(&self) -> Word {
            Word(self.board.word())
        }

        /// What is left of the open time of the breaker `word` says is open,
        /// read with no lock: `None` once it has passed, or once `word` is no
        /// more the one published.
        pub(super) fn open_left(&self, word: Word) -> Option<Duration> {
            let until = self.board.open_until_of(word.0)?;
            left(self.origin + Duration::from_nanos(until))
        }

        /// `at` in nanoseconds past `origin`, none for an instant before it.
        fn nanos(&self, at: Instant) -> u64 {
            u64::try_from(at.saturating_duration_since(self.origin).as_nanos()).unwrap_or(u64::MAX)
        }
    }

    /// One 64-bit cell of a [`Board`]: std's in the process, loom's in the
    /// model that checks the board's orderings.
    pub(super) trait Cell64 {
        fn new(value: u64) -> Self;
        fn load(&self, order: Ordering) -> u64;
        fn store(&self, value: u64, order: Ordering);
    }

    impl Cell64 for AtomicU64 {
        fn new(value: u64) -> Self {
            Self::new(value)
        }

        #[inline]
        fn load(&self, order: Ordering) -> u64 {
            self.load(order)
        }

        #[inline]
        fn store(&self, value: u64, order: Ordering) {
            self.store(value, order);
        }
    }

    /// What the holders of a breaker's lock publish, one after the other, for
    /// a call to read with none: the [`Word`], and the instant an open one's
    /// open time ends at, in nanoseconds past the breaker's origin.
    pub(super) struct Board<C = AtomicU64> {
        word: C,
        open_until: C,
    }

    impl<C: Cell64> Board<C> {
        pub(super) fn new(word: u64) -> Self {
            Self {
                word: C::new(word),
                open_until: C::new(0),
            }
        }

        /// `word` published, after the instant its open time ends at when it
        /// opens: by one holder of the lock at a time.
        #[inline]
        pub(super) fn publish(&self, word: u64, open_until: Option<u64>) {
            // An `until` stored before the word that says it, both Release: a reader of the
            // word reads that `until` or a later one, and one of a later `until` a later word.
            if let Some(until) = open_until
                && self.open_until.load(Ordering::Relaxed) != until
            {
                self.open_until.store(until, Ordering::Release);
            }
            if self.word.load(Ordering::Relaxed) != word {
                self.word.store(word, Ordering::Release);
            }
        }

        /// The word last published.
        #[inline]
        pub(super) fn word(&self) -> u64 {
            self.word.load(Ordering::Acquire)
        }

        /// The open time's end read between `word` and the word published
        /// after it, `None` when the two differ: the end of `word`'s own
        /// opening, or of the one published right after it, still being
        /// published — never an earlier opening's, nor one a later word moved
        /// past.
        #[inline]
        pub(super) fn open_until_of(&self, word: u64) -> Option<u64> {
            let until = self.open_until.load(Ordering::Acquire);
            (self.word() == word).then_some(until)
        }
    }

    /// What a breaker publishes of its state: the epoch, in the bits above
    /// the two its phase takes, and whether it is closed — quiet, when one
    /// more success changes nothing — or open.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(super) struct Word(u64);

    impl Word {
        /// A probe's turn held, or free between two probes: read under the
        /// lock.
        const PROBING: u64 = 0b00;
        const CLOSED: u64 = 0b01;
        const OPEN: u64 = 0b10;
        /// Closed, its window counted with no age, full and holding no
        /// failure: one more success changes neither the window nor what it
        /// reads.
        const QUIET: u64 = 0b11;
        const PHASE: u64 = 0b11;

        fn of(held: &Held, window: usize) -> Self {
            let quiet = held.failed == 0
                && matches!(&held.outcomes, Outcomes::Counted(outcomes) if outcomes.len() == window);
            let phase = match held.phase {
                Phase::Closed if quiet => Self::QUIET,
                Phase::Closed => Self::CLOSED,
                Phase::Open { .. } => Self::OPEN,
                Phase::HalfOpen { .. } => Self::PROBING,
            };
            Self(held.epoch << 2 | phase)
        }

        /// The epoch the breaker was in.
        pub(super) const fn epoch(self) -> u64 {
            self.0 >> 2
        }

        /// Whether the breaker was closed, quiet or not.
        pub(super) const fn closed(self) -> bool {
            self.0 & Self::CLOSED != 0
        }

        /// Whether the breaker was closed and quiet.
        pub(super) const fn quiet(self) -> bool {
            self.0 & Self::PHASE == Self::QUIET
        }

        /// Whether the breaker was open, its open time passed or not.
        pub(super) const fn open(self) -> bool {
            self.0 & Self::PHASE == Self::OPEN
        }
    }

    /// A breaker's state under its lock, its word published as the lock is
    /// let go.
    pub(super) struct Holding<'p> {
        published: &'p Published,
        held: MutexGuard<'p, Held>,
    }

    impl Deref for Holding<'_> {
        type Target = Held;

        fn deref(&self) -> &Held {
            &self.held
        }
    }

    impl DerefMut for Holding<'_> {
        fn deref_mut(&mut self) -> &mut Held {
            &mut self.held
        }
    }

    impl Drop for Holding<'_> {
        fn drop(&mut self) {
            // Still under the lock, which the guard lets go once this returns.
            let published = self.published;
            let open_until = match self.held.phase {
                Phase::Open { until } => Some(published.nanos(until)),
                _ => None,
            };
            let word = Word::of(&self.held, published.window).0;
            published.board.publish(word, open_until);
        }
    }

    #[cfg(test)]
    mod tests {
        use loom::sync::Arc;
        use loom::sync::atomic::AtomicU64 as ModelCell;

        use super::*;

        impl Cell64 for ModelCell {
            fn new(value: u64) -> Self {
                Self::new(value)
            }

            fn load(&self, order: Ordering) -> u64 {
                self.load(order)
            }

            fn store(&self, value: u64, order: Ordering) {
                self.store(value, order);
            }
        }

        /// The end of the open time an opening in `epoch` publishes: each
        /// later one sooner, so a reader told an earlier opening's waits past
        /// the breaker's.
        const fn until_of(epoch: u64) -> u64 {
            1_000 - epoch
        }

        /// What the holders of the lock enter, one epoch each, in the order
        /// the lock lets them: whether each opens.
        const ENTERED: [bool; 3] = [true, false, true];

        /// Every interleaving of the holders of a breaker's lock — opening it,
        /// closing it, opening it again — with a call reading the board with
        /// none, and every value the board's orderings let each load read: a
        /// call meeting the breaker open is told the open time of the opening
        /// its word says, or of the one published right after it, never an
        /// earlier one's, nor one a later word moved past. The word's
        /// Release/Acquire holds the first, the open time's the second.
        #[test]
        fn a_call_reading_the_breaker_with_no_lock_is_told_its_own_openings_wait() {
            loom::model(|| {
                let board = Arc::new(Board::<ModelCell>::new(Word::CLOSED));
                let holders = {
                    let board = Arc::clone(&board);
                    // One thread: the lock orders its holders as one thread's program does.
                    loom::thread::spawn(move || {
                        for (epoch, open) in (1..).zip(ENTERED) {
                            let (phase, until) = if open {
                                (Word::OPEN, Some(until_of(epoch)))
                            } else {
                                (Word::CLOSED, None)
                            };
                            board.publish(Word(epoch << 2 | phase).0, until);
                        }
                    })
                };
                // A thread of its own: loom runs the model's own thread to its first join.
                let reader = loom::thread::spawn(move || {
                    let word = Word(board.word());
                    let told = word.open().then(|| board.open_until_of(word.0)).flatten();
                    (word, told)
                });
                holders.join().expect("no holder panics");
                let (word, told) = reader.join().expect("the call reads");
                let Some(until) = told else {
                    return;
                };
                let epoch = word.epoch();
                let next_opens = usize::try_from(epoch)
                    .ok()
                    .and_then(|next| ENTERED.get(next))
                    .copied()
                    .unwrap_or(false);
                assert!(
                    until == until_of(epoch) || (next_opens && until == until_of(epoch + 1)),
                    "epoch {epoch} told {until}"
                );
            });
        }
    }
}

/// What an attempt failed on, put in words only when something tells it: the
/// breaker's opening, a retry's line or its event.
#[derive(Clone, Copy)]
pub(crate) enum Failure<'a> {
    /// A sentence of the policy's own: a timeout, a panic, a status.
    Said(&'static str),
    /// The error the call answered, told as its chain is
    /// ([`told`]).
    Error(&'a (dyn Error + 'static)),
}

impl Failure<'_> {
    /// What the lines telling the attempt say it failed on — an error as its
    /// chain is told — or the panic of words that panicked, which counts as a
    /// panic of the call's, unwinding on once the failure is counted.
    pub(crate) fn said(self) -> Result<Cow<'static, str>, Unwound> {
        match self {
            Self::Said(said) => Ok(Cow::Borrowed(said)),
            Self::Error(error) => catch_unwind(AssertUnwindSafe(|| told(error))).map(Cow::Owned),
        }
    }
}

/// The payload of a panic a run unwinds on once it has counted it.
pub(crate) type Unwound = Box<dyn Any + Send>;

/// What an attempt comes to in its breaker.
#[derive(Clone, Copy)]
enum Outcome<'e> {
    Success,
    Failure(Failure<'e>),
    /// An answer counting in neither direction.
    Neither,
    /// Given up by its caller before it answered, its call sent.
    Abandoned,
    /// Given up before its call was sent.
    Withdrawn,
}

/// A failure opening the breaker, its words not said yet: the lock is let go
/// for them.
struct Unsaid;

/// What a breaker is built from: its policy's settings.
pub(crate) struct BreakerSettings {
    /// The latest attempts the window counts.
    pub(crate) window: usize,
    /// The failures among them that open the breaker.
    pub(crate) failures: usize,
    /// How long it stays open once they did.
    pub(crate) open_for: Duration,
    /// How long an outcome counts, from its answer, when outcomes have an age.
    pub(crate) age: Option<Duration>,
    /// The probes in a row that close it, succeeding.
    pub(crate) probes: usize,
}

pub(crate) struct Breaker {
    window: usize,
    failures: usize,
    open_for: Duration,
    probes: usize,
    published: Published,
    teller: Arc<Teller>,
}

/// One attempt the breaker admitted, its call not sent yet. Dropped so — the
/// rate refused it, the unit ran out of time, its caller dropped it or the
/// timeout of a policy around it did — it counts for nothing, and a probe
/// hands its turn to the next call with no transition, the dependency never
/// called. It carries the epoch it was admitted in, so an opening or a manual
/// switch since leaves it nothing to change or count.
pub(crate) struct Pass<'b> {
    breaker: &'b Breaker,
    probe: bool,
    epoch: u64,
}

impl<'b> Pass<'b> {
    /// Whether the attempt holds the probe's turn: it took it, and no manual
    /// switch came since — nothing else moves the breaker while a call holds
    /// the turn.
    #[inline]
    pub(crate) fn probes(&self) -> bool {
        self.probe && self.breaker.published.lock().epoch == self.epoch
    }

    /// The attempt's call postponed to `to`, then sent under `bound`: a probe
    /// has answered by their sum at the latest, which every call the breaker
    /// refuses meanwhile is told.
    pub(crate) fn postpone(&self, to: Instant, bound: Duration) {
        if self.probe {
            let until = to + bound;
            self.breaker
                .probe_out(self.epoch, Answers::By { until, bound }, false);
        }
    }

    /// The attempt's call sent under `bound`, which ends at `until` when the
    /// attempt's timer is armed at an instant: a probe's makes the breaker
    /// half-open until it answers, within `bound`.
    #[inline]
    pub(crate) fn send(mut self, bound: Duration, until: Option<Instant>) -> Sent<'b> {
        let probe = std::mem::take(&mut self.probe);
        // Built before the breaker is told: a line panicking out of the half-opening
        // drops it, handing the turn on, where a turn nothing holds is never free again.
        let sent = Sent {
            breaker: self.breaker,
            probe,
            epoch: self.epoch,
            until,
        };
        if probe {
            let until = Instant::now() + bound;
            self.breaker
                .probe_out(self.epoch, Answers::By { until, bound }, true);
        }
        sent
    }
}

impl Drop for Pass<'_> {
    #[inline]
    fn drop(&mut self) {
        if self.probe {
            self.breaker.handed_on(self.epoch, Outcome::Withdrawn);
        }
    }
}

/// One admitted attempt whose call was sent, settled once with its outcome:
/// what it failed on, said before the breaker is asked, or `None` for a
/// success — or ignored, when its answer counts in neither direction. Dropped
/// before it is settled, it is judged by what dropped it:
///
/// - its caller, before its bound ends — a `select!` another branch won, a
///   join dropped: it counts for nothing, and a probe hands its turn to the
///   next call;
/// - a policy around it, giving up at its own bound the attempt this one's
///   call runs in ([`Cut`]): it has timed out, as its own timer would have
///   said had the one around it waited that long;
/// - anything, at or past the instant its bound ends at: it has timed out, as
///   its own timer due there would have said — an edge cutting its unit at
///   that deadline may be polled first, tokio firing one instant's timers in
///   batches that another worker runs between. A bound ends at an instant
///   under a unit's deadline alone ([`until`](Self::until)): sent under none,
///   an attempt its caller drops past its timeout counts for nothing too, and
///   a probe hands its turn on.
///
/// However it ends, it changes and counts nothing once the breaker opened or
/// was switched by hand since it was admitted.
pub(crate) struct Sent<'b> {
    breaker: &'b Breaker,
    probe: bool,
    epoch: u64,
    /// The instant the attempt's bound ends at, when its timer is armed at
    /// one: inside a unit whose edge cuts it at a deadline, and nowhere else.
    until: Option<Instant>,
}

impl Sent<'_> {
    /// Whether the instant the attempt's bound ends at has passed: dropped
    /// now, it has timed out unless its call answered in time.
    pub(crate) fn overdue(&self) -> bool {
        self.until.is_some_and(|until| Instant::now() >= until)
    }

    /// The attempt answered: a success, or a failure on `failure`, put in
    /// words only if it opens the breaker. Words that panic hand the panic
    /// back, the failure counted.
    #[inline]
    pub(crate) fn ended(self, failure: Option<Failure<'_>>) -> Result<(), Unwound> {
        // Settled, it is dropped no more: its drop is an attempt given up
        // unanswered, and reads what gave it up.
        let sent = ManuallyDrop::new(self);
        let outcome = failure.map_or(Outcome::Success, Outcome::Failure);
        sent.breaker.settle(sent.epoch, sent.probe, outcome)
    }

    /// [`ended`](Self::ended) on a success, or on a failure the policy says
    /// in its own words, which cannot panic.
    #[cfg(any(test, feature = "reqwest"))]
    #[inline]
    pub(crate) fn settle(self, failure: Option<&'static str>) {
        let said = self.ended(failure.map(Failure::Said));
        debug_assert!(said.is_ok(), "the policy's own words never panic");
    }

    /// The attempt answered what counts in neither direction — an `Err` its
    /// method's `record_if` refuses: nothing is counted, whenever it answered,
    /// and a probe hands its turn to the next call, its series keeping what
    /// it counted and the breaker half-open as the answer found it, with no
    /// transition — where a probe its caller dropped, the dependency saying
    /// nothing, leaves the breaker open again on no success yet.
    #[inline]
    pub(crate) fn ignore(self) {
        let sent = ManuallyDrop::new(self);
        if sent.probe {
            sent.breaker.handed_on(sent.epoch, Outcome::Neither);
        }
    }

    /// The attempt held past its run, with no borrow of the breaker, by what
    /// its call answered: a response whose body is still to read. No timer
    /// gives it up from here, so a call refused beside a probe held is told
    /// `bound`, the one the attempt was sent under.
    #[cfg(feature = "reqwest")]
    pub(crate) fn hold(self, bound: Duration) -> HeldAttempt {
        let sent = ManuallyDrop::new(self);
        if sent.probe {
            sent.breaker
                .probe_out(sent.epoch, Answers::Held(bound), false);
        }
        HeldAttempt {
            probe: sent.probe,
            epoch: sent.epoch,
        }
    }
}

/// A [`Sent`] attempt held past its run, with no borrow of the breaker, its
/// outcome still to come: taken up again through [`Breaker::resume`], and
/// settled then. Dropped, a probe would keep its turn for good, so what holds
/// it resumes it at its own drop.
#[cfg(feature = "reqwest")]
#[must_use = "a held attempt is settled through `Breaker::resume` alone"]
pub(crate) struct HeldAttempt {
    probe: bool,
    epoch: u64,
}

impl Drop for Sent<'_> {
    #[inline]
    fn drop(&mut self) {
        if Cut::encloses() || self.overdue() {
            let failure = Outcome::Failure(Failure::Said(TIMED_OUT));
            let said = self.breaker.settle(self.epoch, self.probe, failure);
            debug_assert!(said.is_ok(), "the policy's own words never panic");
        } else if self.probe {
            self.breaker.handed_on(self.epoch, Outcome::Abandoned);
        }
    }
}

thread_local! {
    /// Whether this thread is inside a [`Cut`].
    static CUT: Cell<bool> = const { Cell::new(false) };
}

/// The extent of one thread dropping what an attempt given up at its bound
/// held — the future its call made, or the body its answer left to read. An
/// attempt of any policy in flight in there is dropped by that timeout, not
/// by its caller: it counts as timed out in its own breaker
/// ([`Sent`]), so a dependency that never answers opens the breaker naming
/// it, however short the timeout of the policy its caller runs under.
///
/// It is the drop's extent and no more: a future its caller drops sets
/// nothing, and an attempt on a task the call spawned is not dropped in it.
/// One inside another ends as the outer goes on, and a panic out of the drop
/// ends it as it unwinds.
pub(crate) struct Cut {
    /// Whether the thread was inside one already.
    around: bool,
}

impl Cut {
    /// `dropping` run as the drop of what an attempt given up at its bound
    /// held.
    pub(crate) fn dropping<R>(dropping: impl FnOnce() -> R) -> R {
        let _ended = Self {
            around: CUT.replace(true),
        };
        dropping()
    }

    /// Whether what is dropped now is dropped by an attempt's bound passing.
    #[inline]
    pub(crate) fn encloses() -> bool {
        CUT.get()
    }
}

impl Drop for Cut {
    fn drop(&mut self) {
        CUT.set(self.around);
    }
}

/// What moved the breaker, as the line its transition files says it.
#[derive(Clone, Copy)]
enum Moved<'e> {
    /// `failed` failures counted opened it, the last of them saying `error`.
    Failures { failed: usize, error: &'e str },
    /// A probe sent, abandoned or closing it.
    Probe,
    /// A manual switch, opening it for `open_for` or closing it.
    Manual { open_for: Option<Duration> },
}

impl Breaker {
    /// A closed breaker, opening for `open_for` once `failures` of the latest
    /// `window` attempts failed, and closing once `probes` probes in a row
    /// succeeded. With an `age`, an outcome counts until it is older than
    /// that, its own age running from its answer. `teller` tells each
    /// transition.
    pub(crate) fn new(settings: BreakerSettings, teller: Arc<Teller>) -> Self {
        let BreakerSettings {
            window,
            failures,
            open_for,
            age,
            probes,
        } = settings;
        let outcomes = match age {
            None => Outcomes::Counted(VecDeque::with_capacity(window)),
            Some(age) => Outcomes::Aged {
                age,
                counted: VecDeque::with_capacity(window),
            },
        };
        let held = Held {
            phase: Phase::Closed,
            outcomes,
            failed: 0,
            epoch: 0,
            untold: VecDeque::new(),
            telling: false,
        };
        Self {
            window,
            failures,
            open_for,
            probes,
            published: Published::new(held, window),
            teller,
        }
    }

    /// Let a call through — taking the probe's turn once the open time has
    /// passed or between two probes of a series, its call sent now under
    /// `bound` unless it postpones it — or refuse it for the wait the breaker
    /// knows: what is left of the open time, or of the probe's own bound while
    /// another call holds its turn — that whole bound, once the probe's answer
    /// is held past its run. A closed breaker lets the call through, and one
    /// open with time left refuses it, off the published word: no lock.
    #[inline]
    pub(crate) fn admit(&self, bound: Duration) -> Result<Pass<'_>, Duration> {
        let word = self.published.word();
        if word.closed() {
            return Ok(Pass {
                breaker: self,
                probe: false,
                epoch: word.epoch(),
            });
        }
        if word.open()
            && let Some(left) = self.published.open_left(word)
        {
            return Err(left);
        }
        let mut held = self.published.lock();
        let probing = held.phase.admitting(bound)?;
        if let Some(probing) = probing {
            if matches!(held.phase, Phase::HalfOpen { probe: Some(_), .. }) {
                // An overdue probe's turn taken: what it says late counts nowhere.
                held.epoch += 1;
            }
            held.phase = probing;
        }
        Ok(Pass {
            breaker: self,
            probe: probing.is_some(),
            epoch: held.epoch,
        })
    }

    /// The attempt `held` was taken from, one this breaker admitted, to settle:
    /// in the epoch it was admitted in, so an opening or a manual switch since
    /// leaves it nothing to change or count. It ends when it is settled, at no
    /// instant of its own.
    #[cfg(feature = "reqwest")]
    pub(crate) fn resume(&self, held: HeldAttempt) -> Sent<'_> {
        Sent {
            breaker: self,
            probe: held.probe,
            epoch: held.epoch,
            until: None,
        }
    }

    /// How long a call is refused for now, leaving the breaker as it stands:
    /// closed, or open with time left, read off the published word with no
    /// lock.
    pub(crate) fn refusing(&self) -> Option<Duration> {
        let word = self.published.word();
        if word.closed() {
            return None;
        }
        if word.open()
            && let Some(left) = self.published.open_left(word)
        {
            return Some(left);
        }
        self.published.lock().phase.admitting(Duration::ZERO).err()
    }

    /// The state a call arriving now meets, leaving the breaker as it stands.
    pub(crate) fn read(&self) -> BreakerState {
        self.published.lock().phase.state()
    }

    /// What the window counts now, leaving the breaker as it stands.
    pub(crate) fn stats(&self) -> BreakerStats {
        self.published.lock().stats(Instant::now)
    }

    #[cfg(test)]
    fn state(&self) -> &'static str {
        self.published.lock().phase.told().as_str()
    }

    /// Whether the breaker's lock is held now.
    #[cfg(test)]
    pub(crate) fn locked(&self) -> bool {
        self.published.locked()
    }

    /// Open the breaker by hand for `open_for` from now, whatever it was in:
    /// every call is refused that long, then probed as after any opening, and
    /// no attempt admitted before changes it, as after any opening.
    pub(crate) fn open_manually(&self, open_for: Duration) {
        self.switch(
            Phase::Open {
                until: Instant::now() + open_for,
            },
            Some(open_for),
        );
    }

    /// Close the breaker by hand, whatever it was in, its window forgotten,
    /// and no attempt admitted before changing it or counting in the next:
    /// what an opening leaves of the attempts admitted before it.
    pub(crate) fn close_manually(&self) {
        self.switch(Phase::Closed, None);
    }

    /// Enter `phase` by hand, in an epoch of its own, opened for `open_for`
    /// when it opens.
    fn switch(&self, phase: Phase, open_for: Option<Duration>) {
        let mut held = self.published.lock();
        held.epoch += 1;
        let from = held.phase;
        held.phase = phase;
        held.forget();
        self.entered(held, from, Moved::Manual { open_for });
    }

    /// The breaker's state for an attempt admitted in `epoch`: `None` once
    /// the breaker opened or was switched by hand since, which leaves the
    /// attempt nothing to change or count.
    #[inline]
    fn held_since(&self, epoch: u64) -> Option<Holding<'_>> {
        let held = self.published.lock();
        (held.epoch == epoch).then_some(held)
    }

    /// The probe out, of an attempt admitted in `epoch`, has answered as
    /// `answers` says, its call sent when `sending`: the first probe sent of
    /// a series makes the breaker half-open, which it files.
    fn probe_out(&self, epoch: u64, answers: Answers, sending: bool) {
        let Some(mut held) = self.held_since(epoch) else {
            return;
        };
        let from = held.phase;
        if let Phase::HalfOpen { sent, probe, .. } = &mut held.phase {
            *probe = Some(answers);
            *sent |= sending;
            if held.phase.told() != from.told() {
                self.entered(held, from, Moved::Probe);
            }
        }
    }

    /// What an attempt admitted in `epoch` came to, the probe or not: a
    /// probe opens the breaker again on a failure, closes it as the last of
    /// its series to succeed, or hands its turn to the next call; any other
    /// attempt counts in the window of the closed breaker that admitted it —
    /// and for nothing once the breaker opened or was switched by hand since.
    /// A success the published word says is voided so, or is one more in a
    /// quiet window, changes nothing and takes no lock.
    ///
    /// A failure is put in words only when it opens the breaker, and never
    /// under its lock: the lock is let go, the words said, and the breaker
    /// read again. Words that panic count as [`PANICKED`], and the panic is
    /// handed back for the attempt's run to unwind on.
    #[inline]
    fn settle(&self, epoch: u64, probe: bool, outcome: Outcome<'_>) -> Result<(), Unwound> {
        if !probe {
            match outcome {
                Outcome::Success => {
                    let word = self.published.word();
                    if word.epoch() != epoch || word.quiet() {
                        return Ok(());
                    }
                }
                Outcome::Failure(_) => {}
                Outcome::Neither | Outcome::Abandoned | Outcome::Withdrawn => return Ok(()),
            }
        }
        self.settle_held(epoch, probe, outcome)
    }

    /// [`settle`](Self::settle) under the lock.
    #[inline(never)]
    fn settle_held(&self, epoch: u64, probe: bool, outcome: Outcome<'_>) -> Result<(), Unwound> {
        let mut said = None;
        let mut unwound = None;
        while let Some(held) = self.held_since(epoch) {
            let Err(Unsaid) = self.moved(held, probe, outcome, said.as_deref()) else {
                break;
            };
            let Outcome::Failure(failure) = outcome else {
                unreachable!("only a failure opens the breaker");
            };
            said = Some(failure.said().unwrap_or_else(|payload| {
                unwound = Some(payload);
                Cow::Borrowed(PANICKED)
            }));
        }
        unwound.map_or(Ok(()), Err)
    }

    /// The one transition `outcome` makes of the breaker `held` holds, for
    /// the probe or another attempt — or, for a failure that opens it with
    /// no words `said` yet, none: the lock is let go for them to be said.
    fn moved(
        &self,
        mut held: Holding<'_>,
        probe: bool,
        outcome: Outcome<'_>,
        said: Option<&str>,
    ) -> Result<(), Unsaid> {
        let from = held.phase;
        let moved = match (from, outcome) {
            (Phase::HalfOpen { .. }, Outcome::Failure(_)) if probe => {
                let error = said.ok_or(Unsaid)?;
                self.open(&mut held);
                Moved::Failures { failed: 1, error }
            }
            (Phase::HalfOpen { passed, .. }, Outcome::Success)
                if probe && passed + 1 >= self.probes =>
            {
                held.phase = Phase::Closed;
                held.forget();
                Moved::Probe
            }
            // The next call probes instead, the series keeping what it counted —
            // and on no success yet, a probe abandoned once sent leaves it open again.
            (Phase::HalfOpen { passed, sent, .. }, handed_on) if probe => {
                let abandoned = passed == 0 && matches!(handed_on, Outcome::Abandoned);
                held.phase = Phase::HalfOpen {
                    passed: passed + usize::from(matches!(handed_on, Outcome::Success)),
                    sent: sent && !abandoned,
                    probe: None,
                };
                if held.phase.told() == from.told() {
                    return Ok(());
                }
                Moved::Probe
            }
            (Phase::Closed, Outcome::Success | Outcome::Failure(_)) if !probe => {
                let failure = matches!(outcome, Outcome::Failure(_));
                if failure
                    && said.is_none()
                    && held.failing(self.window, Instant::now) >= self.failures
                {
                    return Err(Unsaid);
                }
                let failed = held.count(self.window, failure, Instant::now);
                let Some(error) = said.filter(|_| failure && failed >= self.failures) else {
                    return Ok(());
                };
                self.open(&mut held);
                held.forget();
                Moved::Failures { failed, error }
            }
            _ => return Ok(()),
        };
        self.entered(held, from, moved);
        Ok(())
    }

    /// A probe of an attempt admitted in `epoch` handed on with no answer to
    /// count, or one counting in neither direction: words are never said.
    fn handed_on(&self, epoch: u64, outcome: Outcome<'_>) {
        let said = self.settle(epoch, true, outcome);
        debug_assert!(said.is_ok(), "only a failure is put in words");
    }

    /// Open `held` for the whole open time, in an epoch of its own: the
    /// attempts admitted before it change and count nothing from here on.
    fn open(&self, held: &mut Held) {
        held.epoch += 1;
        held.phase = Phase::Open {
            until: Instant::now() + self.open_for,
        };
    }

    /// The transition `held` just made out of `from`, queued behind the ones
    /// still to tell — with what failures opened it when they did and what
    /// the last of them failed on, and that it was manual, with the open time
    /// it was given, when a manual switch moved it — then the breaker's lock
    /// let go, and the queue told by this thread unless another is telling it.
    ///
    /// Nothing is told under the breaker's lock: a line runs the app's
    /// `tracing` subscriber, a counter waits for the meter's pipeline at its
    /// first record, and an event wakes its readers, any of which may read or
    /// switch this breaker. The queue is filled under that lock and told by
    /// one thread at a time, so the lines, the counts and the events say the
    /// transitions in the order they took effect, each once.
    ///
    /// A line, a count or a wake that panics unwinds out of here, the breaker
    /// already in the phase it entered and the turn to tell given back
    /// ([`Telling`]) — but from a destructor run as its thread unwinds, an
    /// attempt dropped by a panic, where it ends here: the transition it was
    /// telling is lost as any other it panics out of, and those behind it are
    /// told with the next one made.
    #[inline(never)]
    fn entered(&self, mut held: Holding<'_>, from: Phase, moved: Moved<'_>) {
        let (failed, error, open_for, manual) = match moved {
            Moved::Failures { failed, error } => (
                Some((failed, self.window)),
                Some(error),
                Some(self.open_for),
                false,
            ),
            Moved::Probe => (None, None, None, false),
            Moved::Manual { open_for } => (None, None, open_for, true),
        };
        let to = held.phase.told();
        held.untold.push_back(Transition {
            from: from.told(),
            to,
            failed,
            error: error.map(Box::from),
            open_for,
            manual,
        });
        if std::mem::replace(&mut held.telling, true) {
            return;
        }
        drop(held);
        if std::thread::panicking() {
            // Reached from a destructor as the thread unwinds: a second panic
            // out of it would abort the process. The hook has filed it.
            let _contained = catch_unwind(AssertUnwindSafe(|| self.tell()));
        } else {
            self.tell();
        }
    }

    /// Tell each transition still to tell, oldest first, as the one thread
    /// whose turn it is: until none is left, the ones other threads queue
    /// meanwhile included — and the ones this thread queues from inside a
    /// line or a wake, which are told once that one is.
    fn tell(&self) {
        let turn = Telling(self);
        while let Some(transition) = self.untold() {
            self.teller.transition(&transition);
        }
        // The turn was given up as the queue was found empty: dropped, the
        // guard would take it from the thread telling since.
        std::mem::forget(turn);
    }

    /// The oldest transition still to tell — or none, and the turn to tell
    /// them given up in the same hold of the lock, so the next one queued
    /// finds it free.
    fn untold(&self) -> Option<Transition> {
        let mut held = self.published.lock();
        let next = held.untold.pop_front();
        held.telling = next.is_some();
        next
    }
}

/// The turn to tell a breaker's transitions, given back when a line, a count
/// or an event panics out of it: the transitions left are told with the next
/// one made, where a turn never given back would tell none again.
struct Telling<'b>(&'b Breaker);

impl Drop for Telling<'_> {
    fn drop(&mut self) {
        self.0.published.lock().telling = false;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use parking_lot::Mutex;

    use super::*;
    use crate::TARGET;
    use crate::events::ResilienceEvent;
    use crate::teller::MOVED;

    const OPEN_FOR: Duration = Duration::from_secs(30);
    /// What each call is sent under, but where a test sets its own.
    const BOUND: Duration = Duration::from_secs(5);

    fn breaker(window: usize, failures: usize) -> Breaker {
        Breaker::new(
            BreakerSettings {
                window,
                failures,
                open_for: OPEN_FOR,
                age: None,
                probes: 1,
            },
            Arc::new(Teller::new("upstream")),
        )
    }

    /// What a failed attempt of these tests says.
    const DOWN: &str = "the upstream refused: connection reset by peer";

    fn attempt(breaker: &Breaker, failed: bool) {
        breaker
            .admit(BOUND)
            .expect("a closed breaker admits")
            .send(BOUND, None)
            .settle(failed.then_some(DOWN));
    }

    fn states(lines: &nest_rs_testing::LogCapture) -> Vec<String> {
        lines
            .find(TARGET, MOVED)
            .iter()
            .map(|event| event.field("state").unwrap_or_default())
            .collect()
    }

    fn refused_for(breaker: &Breaker) -> Duration {
        match breaker.admit(BOUND) {
            Err(retry_after) => retry_after,
            Ok(_) => panic!("the breaker refuses"),
        }
    }

    /// An error counting how often it is said.
    #[derive(Debug)]
    struct Counted(AtomicUsize);

    impl std::fmt::Display for Counted {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            self.0.fetch_add(1, Ordering::Relaxed);
            f.write_str(DOWN)
        }
    }

    impl Error for Counted {}

    /// Two threads failing at once on the failure that opens the breaker,
    /// each finding it would open it, let its lock go to say their error: it
    /// opens once, and the other's failure, voided by that opening, counts
    /// nowhere.
    #[test]
    fn two_threads_failing_at_the_threshold_open_the_breaker_once() {
        for _ in 0..200 {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("a runtime");
            let _entered = runtime.enter();
            let breaker = breaker(3, 2);
            attempt(&breaker, true);
            let error = Counted(AtomicUsize::new(0));
            let together = std::sync::Barrier::new(2);
            std::thread::scope(|scope| {
                for _ in 0..2 {
                    scope.spawn(|| {
                        let _entered = runtime.enter();
                        let sent = breaker
                            .admit(BOUND)
                            .expect("a closed breaker admits")
                            .send(BOUND, None);
                        together.wait();
                        let said = sent.ended(Some(Failure::Error(&error)));
                        assert!(said.is_ok());
                    });
                }
            });
            assert_eq!(breaker.state(), "open");
            assert_eq!(breaker.published.lock().epoch, 1, "opened once");
            let said = error.0.load(Ordering::Relaxed);
            assert!(
                (1..=2).contains(&said),
                "said by the threads it opened for: {said}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn it_opens_once_the_window_holds_enough_failures() {
        let breaker = breaker(4, 2);
        attempt(&breaker, true);
        attempt(&breaker, false);
        assert_eq!(breaker.state(), "closed", "one failure of two");
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open");
        assert_eq!(refused_for(&breaker), OPEN_FOR);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failure_older_than_the_window_counts_no_more() {
        let breaker = breaker(2, 2);
        attempt(&breaker, true);
        attempt(&breaker, false);
        attempt(&breaker, false);
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "closed",
            "the first failure left the window before the second came",
        );
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open");
    }

    #[tokio::test(start_paused = true)]
    async fn an_open_breaker_says_how_long_is_left_then_lets_one_probe_through() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(refused_for(&breaker), Duration::from_secs(20));
        tokio::time::advance(Duration::from_secs(20)).await;
        let Ok(probe) = breaker.admit(BOUND) else {
            panic!("the open time passed");
        };
        assert_eq!(
            (breaker.state(), refused_for(&breaker)),
            ("open", BOUND),
            "the probe's turn is taken, its call not sent yet",
        );
        let probe = probe.send(BOUND, None);
        assert_eq!(breaker.state(), "half_open");
        assert_eq!(
            refused_for(&breaker),
            BOUND,
            "one probe at a time; another call waits for its answer",
        );
        probe.settle(None);
        assert_eq!(breaker.state(), "closed");
        attempt(&breaker, false);
    }

    /// A call refused while another holds the probe's turn is told what is
    /// left of the bound that probe handed the breaker — when it took its
    /// turn, when it postponed its call, and when it sent it — and, past it,
    /// a tick: the probe is being given up, and still holds its turn, until a
    /// whole bound of its own more has passed.
    #[tokio::test(start_paused = true)]
    async fn a_call_refused_beside_the_probe_is_told_what_is_left_of_the_probes_own_bound() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let second = Duration::from_secs(1);
        let probe = breaker.admit(2 * second).expect("the open time passed");
        assert_eq!(refused_for(&breaker), 2 * second, "sent now, under 2 s");
        probe.postpone(Instant::now() + 10 * second, 2 * second);
        assert_eq!(
            refused_for(&breaker),
            12 * second,
            "sent in 10 s, under 2 s"
        );
        tokio::time::advance(10 * second).await;
        let probe = probe.send(3 * second, None);
        assert_eq!(refused_for(&breaker), 3 * second, "sent, under 3 s");
        tokio::time::advance(second).await;
        assert_eq!(refused_for(&breaker), 2 * second);
        tokio::time::advance(4 * second).await;
        assert_eq!(refused_for(&breaker), OVERDUE, "a second past its bound");
        probe.settle(None);
        assert_eq!(breaker.state(), "closed");
    }

    /// A probe whose answer was due a whole bound of its own ago — its run
    /// kept and never polled again, which no timer gives up — hands its turn
    /// to the next call: that call probes, and the late answer, a success or
    /// a failure, counts nowhere, the breaker as the new probe leaves it. One
    /// due a tick short of that still holds the turn.
    #[tokio::test(start_paused = true)]
    async fn an_overdue_probe_gives_its_turn_up_and_its_late_answer_counts_nothing() {
        for late in [None, Some(DOWN)] {
            let breaker = breaker(1, 1);
            attempt(&breaker, true);
            tokio::time::advance(OPEN_FOR).await;
            let kept = breaker
                .admit(BOUND)
                .expect("the open time passed")
                .send(BOUND, None);
            tokio::time::advance(2 * BOUND - Duration::from_millis(1)).await;
            assert_eq!(refused_for(&breaker), OVERDUE, "a tick short, it holds");
            tokio::time::advance(Duration::from_millis(1)).await;
            assert!(breaker.refusing().is_none(), "the turn is free");
            let next = breaker.admit(BOUND).expect("the turn handed on");
            assert!(next.probes(), "the arriving call probes");
            kept.settle(late);
            assert_eq!(
                breaker.state(),
                "half_open",
                "{late:?} came late, and counted nowhere"
            );
            next.send(BOUND, None).settle(None);
            assert_eq!(breaker.state(), "closed", "the new probe closed it");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_probe_opens_the_breaker_again_for_its_whole_open_time() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let Ok(probe) = breaker.admit(BOUND) else {
            panic!("the open time passed");
        };
        probe.send(BOUND, None).settle(Some(DOWN));
        assert_eq!(breaker.state(), "open");
        assert_eq!(refused_for(&breaker), OPEN_FOR);
    }

    /// A probe given up before its call is sent hands its turn on as though it
    /// never took it, filing nothing; one given up after it, the breaker open
    /// again for the next call to probe, files that transition.
    #[tokio::test(start_paused = true)]
    async fn a_probe_given_up_hands_its_turn_to_the_next_call() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        drop(breaker.admit(BOUND).expect("the open time passed"));
        assert_eq!(breaker.state(), "open");
        let sent = breaker
            .admit(BOUND)
            .expect("the next call probes at once")
            .send(BOUND, None);
        drop(sent);
        assert_eq!(breaker.state(), "open");
        attempt(&breaker, false);
        assert_eq!(breaker.state(), "closed", "the next call probed again");
        assert_eq!(
            states(&lines),
            ["open", "half_open", "open", "half_open", "closed"]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_answer_arriving_once_another_call_opened_the_breaker_counts_for_nothing() {
        let breaker = breaker(1, 1);
        let Ok(late) = breaker.admit(BOUND) else {
            panic!("closed");
        };
        let late = late.send(BOUND, None);
        attempt(&breaker, true);
        late.settle(None);
        assert_eq!(breaker.state(), "open", "a late success closes nothing");
        assert_eq!(breaker.stats(), NOTHING, "nor counts in a window");
    }

    #[tokio::test(start_paused = true)]
    async fn a_closed_breaker_starts_its_window_afresh() {
        let breaker = breaker(3, 2);
        attempt(&breaker, true);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let Ok(probe) = breaker.admit(BOUND) else {
            panic!("the open time passed");
        };
        probe.send(BOUND, None).settle(None);
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "closed",
            "the failures before it opened are forgotten",
        );
    }

    /// Each transition adds one to the counter, by the instance and the state it
    /// entered, on the meter `OpenTelemetry::init()` installs — after the
    /// breaker was built, too — and a probe ignored, which moves nothing, adds
    /// none, nor the probe sent after it.
    #[cfg(feature = "opentelemetry")]
    #[tokio::test(start_paused = true)]
    async fn each_transition_is_counted_by_its_instance_and_the_state_entered() {
        use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};

        use crate::counters::{INSTANCE_ATTRIBUTE, STATE_ATTRIBUTE, TRANSITIONS};
        use opentelemetry_sdk::metrics::{
            InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        };

        let breaker = breaker(1, 1);
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());

        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        drop(
            breaker
                .admit(BOUND)
                .expect("a probe given up before its call is sent"),
        );
        probe(&breaker).send(BOUND, None).ignore();
        probe(&breaker).send(BOUND, None).settle(Some(DOWN));
        provider.force_flush().expect("the export");

        let exported = exporter
            .get_finished_metrics()
            .expect("the exported metrics");
        let mut counted: Vec<(String, u64)> = exported
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .filter(|metric| metric.name() == TRANSITIONS)
            .flat_map(|metric| {
                let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                    panic!("a counter: {metric:?}");
                };
                sum.data_points()
                    .map(|point| {
                        let attribute = |key: &str| {
                            point
                                .attributes()
                                .find(|kv| kv.key.as_str() == key)
                                .map(|kv| kv.value.to_string())
                                .unwrap_or_default()
                        };
                        assert_eq!(attribute(INSTANCE_ATTRIBUTE), "upstream");
                        (attribute(STATE_ATTRIBUTE), point.value())
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        counted.sort();
        assert_eq!(
            counted,
            [("half_open".to_owned(), 1), ("open".to_owned(), 2)],
            "opened, probed, opened again"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn each_transition_is_a_warn_naming_the_instance_and_the_state_entered() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let Ok(probe) = breaker.admit(BOUND) else {
            panic!("the open time passed");
        };
        probe.send(BOUND, None).settle(None);
        let said: Vec<(String, String)> = lines
            .find(TARGET, MOVED)
            .iter()
            .map(|event| {
                assert_eq!(event.level, "warn", "{event:?}");
                assert_eq!(event.field("instance").as_deref(), Some("upstream"));
                (
                    event.field("state").unwrap_or_default(),
                    event.field("open_ms").unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            said,
            [
                ("open".to_owned(), "30000".to_owned()),
                ("half_open".to_owned(), String::new()),
                ("closed".to_owned(), String::new()),
            ]
        );
    }

    /// An opening by failures says what the failure that opened it failed on —
    /// the last of the window's, a failed probe's, and the timeout's sentence
    /// for a probe its edge cut first at its bound — and no other transition
    /// says an error: a probe sent, abandoned or closing it, a manual switch.
    #[tokio::test(start_paused = true)]
    async fn an_opening_by_failures_says_what_the_last_of_them_failed_on_and_no_other_transition() {
        const REFUSED: &str = "the upstream refused: 503";
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(2, 2);
        attempt(&breaker, true);
        breaker
            .admit(BOUND)
            .expect("a closed breaker admits")
            .send(BOUND, None)
            .settle(Some(REFUSED));
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(Some(DOWN));
        tokio::time::advance(OPEN_FOR).await;
        drop(probe(&breaker).send(BOUND, None));
        let cut = probe(&breaker).send(BOUND, Some(Instant::now() + BOUND));
        tokio::time::advance(BOUND).await;
        drop(cut);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(None);
        breaker.open_manually(OPEN_FOR);
        breaker.close_manually();

        let said: Vec<(String, Option<String>)> = lines
            .find(TARGET, MOVED)
            .iter()
            .map(|event| {
                (
                    event.field("state").unwrap_or_default(),
                    event.field("error"),
                )
            })
            .collect();
        let said: Vec<(&str, Option<&str>)> = said
            .iter()
            .map(|(state, error)| (state.as_str(), error.as_deref()))
            .collect();
        assert_eq!(
            said,
            [
                ("open", Some(REFUSED)),
                ("half_open", None),
                ("open", Some(DOWN)),
                ("half_open", None),
                ("open", None),
                ("half_open", None),
                ("open", Some(TIMED_OUT)),
                ("half_open", None),
                ("closed", None),
                ("open", None),
                ("closed", None),
            ],
            "failures, a failed probe, a probe abandoned, a probe cut at its bound, a probe \
             closing it, then a hand opening and closing it"
        );
    }

    const AGE: Duration = Duration::from_secs(60);

    /// A breaker forgetting an outcome older than `age`, opening once
    /// `failures` of the latest `window` attempts failed.
    fn aged(window: usize, failures: usize, age: Duration) -> Breaker {
        Breaker::new(
            BreakerSettings {
                window,
                failures,
                open_for: OPEN_FOR,
                age: Some(age),
                probes: 1,
            },
            Arc::new(Teller::new("upstream")),
        )
    }

    /// A breaker one failure opens, and `probes` probes in a row close.
    fn probing(probes: usize) -> Breaker {
        Breaker::new(
            BreakerSettings {
                window: 1,
                failures: 1,
                open_for: OPEN_FOR,
                age: None,
                probes,
            },
            Arc::new(Teller::new("upstream")),
        )
    }

    /// The next call, taking the probe's turn.
    fn probe(breaker: &Breaker) -> Pass<'_> {
        let pass = breaker.admit(BOUND).expect("the probe's turn is free");
        assert!(pass.probes(), "the call takes the probe's turn");
        pass
    }

    /// Each transition `events` was told and has not answered yet, as the
    /// phase it left, the one it entered and whether a hand moved it.
    fn told(
        events: &mut crate::events::ResilienceEvents,
    ) -> Vec<(BreakerPhase, BreakerPhase, bool)> {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        let mut told = Vec::new();
        loop {
            let mut next = std::pin::pin!(events.next());
            let Poll::Ready(Some(event)) =
                next.as_mut().poll(&mut Context::from_waker(Waker::noop()))
            else {
                return told;
            };
            let ResilienceEvent::Transition { from, to, manual } = event else {
                panic!("a transition: {event:?}");
            };
            told.push((from, to, manual));
        }
    }

    /// Each transition's line, as `state`, then `manual`, `open_ms` and
    /// `failed` where it carries them.
    fn said(lines: &nest_rs_testing::LogCapture) -> Vec<String> {
        lines
            .find(TARGET, MOVED)
            .iter()
            .map(|event| {
                assert_eq!(event.level, "warn", "{event:?}");
                assert_eq!(event.field("instance").as_deref(), Some("upstream"));
                let mut said = event.field("state").unwrap_or_default();
                if let Some(manual) = event.field("manual") {
                    assert_eq!(manual, "true", "{event:?}");
                    said.push_str(" manual");
                }
                for field in ["open_ms", "failed", "window"] {
                    if let Some(value) = event.field(field) {
                        said.push_str(&format!(" {field}={value}"));
                    }
                }
                said
            })
            .collect()
    }

    /// An open time under a millisecond, which `breaker_open` and a manual
    /// opening take in code above zero, is filed in whole milliseconds rounded
    /// up, `1`, never the `0` of no open time, whether failures or a hand
    /// opened the breaker for it.
    #[tokio::test(start_paused = true)]
    async fn an_open_time_under_a_millisecond_is_filed_as_one_never_as_zero() {
        let lines = nest_rs_testing::LogCapture::install();
        let under = Duration::from_micros(500);
        let breaker = Breaker::new(
            BreakerSettings {
                window: 1,
                failures: 1,
                open_for: under,
                age: None,
                probes: 1,
            },
            Arc::new(Teller::new("upstream")),
        );
        attempt(&breaker, true);
        breaker.open_manually(under);
        assert_eq!(
            said(&lines),
            ["open open_ms=1 failed=1 window=1", "open manual open_ms=1"]
        );
    }

    /// An open time under a millisecond is read off the clock, never a timer:
    /// every call before its end is refused, told what is left of it, and the
    /// first at its end probes.
    #[tokio::test(start_paused = true)]
    async fn an_open_time_under_a_millisecond_refuses_until_its_end_and_probes_at_it() {
        let breaker = breaker(1, 1);
        let under = Duration::from_micros(500);
        breaker.open_manually(under);
        let tick = Duration::from_micros(1);
        tokio::time::advance(under - tick).await;
        assert_eq!(refused_for(&breaker), tick, "a microsecond before its end");
        tokio::time::advance(tick).await;
        probe(&breaker).send(BOUND, None).settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);
    }

    /// A breaker one failure short of opening, idle past the age, does not
    /// open on the next failure: the window forgets its old outcomes as it
    /// counts the new one, with no call arriving in between.
    #[tokio::test(start_paused = true)]
    async fn a_breaker_one_failure_short_and_idle_past_the_age_does_not_open_on_the_next_failure() {
        let breaker = aged(4, 2, AGE);
        attempt(&breaker, true);
        tokio::time::advance(AGE + Duration::from_millis(1)).await;
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "closed",
            "the first failure is past the age"
        );
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open", "two failures inside the age");
    }

    /// The failing call that would open it is sent before the idle time and
    /// answers after it: its outcome, counted at its answer, finds the failure
    /// before it past the age.
    #[tokio::test(start_paused = true)]
    async fn a_failure_sent_before_the_idle_time_and_answering_after_it_opens_nothing() {
        let breaker = aged(4, 2, AGE);
        attempt(&breaker, true);
        let late = breaker.admit(BOUND).expect("closed").send(BOUND, None);
        tokio::time::advance(AGE + Duration::from_millis(1)).await;
        late.settle(Some(DOWN));
        assert_eq!(breaker.state(), "closed");
    }

    /// Every outcome past the age is forgotten as the next is counted, however
    /// many: a failure behind two successes as old leaves with them.
    #[tokio::test(start_paused = true)]
    async fn every_outcome_past_the_age_is_forgotten_as_the_next_is_counted() {
        let breaker = aged(10, 2, AGE);
        attempt(&breaker, false);
        attempt(&breaker, false);
        attempt(&breaker, true);
        tokio::time::advance(AGE + Duration::from_millis(1)).await;
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "closed",
            "the three outcomes before it are past the age"
        );
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open", "two failures inside the age");
    }

    /// A success past the age leaves the window taking no failure with it: a
    /// failure inside the age still counts.
    #[tokio::test(start_paused = true)]
    async fn a_success_past_the_age_takes_no_failure_with_it() {
        let breaker = aged(10, 2, AGE);
        attempt(&breaker, false);
        tokio::time::advance(AGE / 2).await;
        attempt(&breaker, true);
        tokio::time::advance(AGE / 2 + Duration::from_secs(1)).await;
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "open",
            "the success is past the age, the failure after it 31 s old"
        );
    }

    /// An outcome counts until it is older than the age, the instant it is as
    /// old as the age included.
    #[tokio::test(start_paused = true)]
    async fn an_outcome_counts_until_it_is_older_than_the_age() {
        let breaker = aged(4, 2, AGE);
        attempt(&breaker, true);
        tokio::time::advance(AGE).await;
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "open",
            "the first failure is as old as the age"
        );
    }

    /// An age of no time, which the config refuses, forgets an outcome once the
    /// clock has moved past its answer: the window counts only the outcomes
    /// answered at one instant, as the age's floor says.
    #[tokio::test(start_paused = true)]
    async fn an_age_of_no_time_counts_only_the_outcomes_answered_at_one_instant() {
        let breaker = aged(4, 2, Duration::ZERO);
        attempt(&breaker, true);
        tokio::time::advance(Duration::from_millis(1)).await;
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "closed",
            "the first failure was answered before the clock moved"
        );
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "open",
            "two failures answered at one instant"
        );
    }

    /// A breaker open for no time, which the config refuses, lets the next call
    /// through at once as its probe, every call beside it refused, as the open
    /// time's floor says.
    #[tokio::test(start_paused = true)]
    async fn a_breaker_open_for_no_time_lets_the_next_call_through_at_once_as_its_probe() {
        let breaker = Breaker::new(
            BreakerSettings {
                window: 1,
                failures: 1,
                open_for: Duration::ZERO,
                age: None,
                probes: 1,
            },
            Arc::new(Teller::new("upstream")),
        );
        attempt(&breaker, true);
        let _probe = probe(&breaker);
        assert_eq!(refused_for(&breaker), BOUND, "beside the probe");
    }

    /// Outcomes with an age still leave the window by its length.
    #[tokio::test(start_paused = true)]
    async fn an_aged_window_still_holds_its_latest_attempts_alone() {
        let breaker = aged(2, 2, AGE);
        attempt(&breaker, true);
        attempt(&breaker, false);
        attempt(&breaker, false);
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "closed",
            "the first failure left the window before the second came",
        );
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open");
    }

    /// A window holds its latest attempts and no more, with an age or without:
    /// of a failure, a success and a failure, a window of two holds the last
    /// two, one failure short of opening, where a window of three would open.
    #[tokio::test(start_paused = true)]
    async fn a_window_holds_its_latest_attempts_and_no_more_with_an_age_or_without() {
        for (breaker, kind) in [(breaker(2, 2), "no age"), (aged(2, 2, AGE), "an age")] {
            attempt(&breaker, true);
            attempt(&breaker, false);
            attempt(&breaker, true);
            assert_eq!(
                breaker.state(),
                "closed",
                "with {kind}: the first failure left"
            );
            attempt(&breaker, true);
            assert_eq!(breaker.state(), "open", "with {kind}");
        }
    }

    /// What a breaker counts once it forgot its window: one failure leaves it
    /// closed and a second opens it, no failure before counting.
    fn counts_afresh(breaker: &Breaker, forgot: &str) {
        assert_eq!(breaker.read(), BreakerState::Closed, "{forgot}");
        attempt(breaker, true);
        assert_eq!(breaker.state(), "closed", "{forgot}: one failure since");
        attempt(breaker, true);
        assert_eq!(breaker.state(), "open", "{forgot}: two failures since");
    }

    /// A breaker with an age forgets its window as one without does, whatever
    /// closes it: a manual closing, or the probe after a manual opening or
    /// after the failures that opened it.
    #[tokio::test(start_paused = true)]
    async fn a_breaker_with_an_age_forgets_its_window_whatever_closes_it() {
        let breaker = aged(2, 2, AGE);
        attempt(&breaker, true);
        breaker.close_manually();
        counts_afresh(&breaker, "closed by hand");

        let breaker = aged(2, 2, AGE);
        attempt(&breaker, true);
        breaker.open_manually(OPEN_FOR);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(None);
        counts_afresh(&breaker, "opened by hand, closed by its probe");

        let breaker = aged(2, 2, AGE);
        attempt(&breaker, true);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(None);
        counts_afresh(&breaker, "opened by failures, closed by its probe");
    }

    /// With no age, the window holds a byte per outcome and no instant, and an
    /// outcome counts however old it is.
    #[tokio::test(start_paused = true)]
    async fn a_breaker_with_no_age_keeps_no_instant_and_counts_an_outcome_however_old() {
        let breaker = breaker(4, 2);
        assert!(matches!(
            breaker.published.lock().outcomes,
            Outcomes::Counted(_)
        ));
        attempt(&breaker, true);
        tokio::time::advance(Duration::from_secs(24 * 60 * 60)).await;
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open");
    }

    /// A window with no age counts an outcome without reading the clock it is
    /// handed, which only an age reads.
    #[test]
    fn a_window_with_no_age_counts_an_outcome_reading_no_clock() {
        let breaker = breaker(4, 2);
        let mut held = breaker.published.lock();
        let unread = || -> Instant { panic!("a window with no age reads no clock to count") };
        assert_eq!(held.count(4, true, unread), 1);
        assert_eq!(held.count(4, false, unread), 1);
        assert_eq!(held.count(4, true, unread), 2);
    }

    /// A breaker asking three probes closes on the third in a row, each let
    /// through alone, and its lines say it opened, probed and closed: nothing
    /// between two probes of the series, which changed no state.
    #[tokio::test(start_paused = true)]
    async fn a_series_of_probes_one_at_a_time_closes_the_breaker_on_its_last() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = probing(3);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        for nth in 1..=3 {
            assert_eq!(breaker.read(), BreakerState::HalfOpen, "before probe {nth}");
            let pending = probe(&breaker);
            assert_eq!(refused_for(&breaker), BOUND, "probe {nth} holds the turn");
            let sent = pending.send(BOUND, None);
            assert_eq!(refused_for(&breaker), BOUND, "probe {nth}'s call is out");
            sent.settle(None);
        }
        assert_eq!(breaker.read(), BreakerState::Closed);
        attempt(&breaker, false);
        assert_eq!(states(&lines), ["open", "half_open", "closed"]);
    }

    /// A failed probe of a series opens the breaker again for its whole open
    /// time, and the series starts over.
    #[tokio::test(start_paused = true)]
    async fn a_failed_probe_of_a_series_opens_the_breaker_again_and_the_series_starts_over() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = probing(2);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(None);
        probe(&breaker).send(BOUND, None).settle(Some(DOWN));
        assert_eq!(refused_for(&breaker), OPEN_FOR);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(None);
        assert_eq!(
            breaker.read(),
            BreakerState::HalfOpen,
            "one probe of two since it opened again"
        );
        probe(&breaker).send(BOUND, None).settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);
        assert_eq!(
            states(&lines),
            ["open", "half_open", "open", "half_open", "closed"]
        );
    }

    /// A probe of a series dropped before it answers, its call sent or not,
    /// says nothing of the dependency: it counts as neither a success nor a
    /// failure, the series keeps its count, and nothing is filed.
    #[tokio::test(start_paused = true)]
    async fn a_probe_of_a_series_dropped_before_it_answers_counts_as_neither_and_keeps_the_count() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = probing(3);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(None);
        drop(probe(&breaker));
        assert_eq!(breaker.read(), BreakerState::HalfOpen, "dropped unsent");
        drop(probe(&breaker).send(BOUND, None));
        assert_eq!(breaker.read(), BreakerState::HalfOpen, "dropped sent");
        probe(&breaker).send(BOUND, None).settle(None);
        assert_eq!(breaker.read(), BreakerState::HalfOpen, "two of three");
        probe(&breaker).send(BOUND, None).settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);
        assert_eq!(states(&lines), ["open", "half_open", "closed"]);
    }

    /// An attempt sent under a bound ending at an instant and dropped, never
    /// settled, at or past it has timed out: a failure in the window, a
    /// failed probe opening the breaker again for its whole open time. One
    /// dropped before that instant counts for nothing, its probe handed on.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_dropped_before_it_is_settled_has_timed_out_at_its_bounds_end() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = probing(2);
        let until = || Some(Instant::now() + BOUND);
        drop(breaker.admit(BOUND).expect("closed").send(BOUND, until()));
        assert_eq!(breaker.read(), BreakerState::Closed, "dropped early");
        let sent = breaker.admit(BOUND).expect("closed").send(BOUND, until());
        tokio::time::advance(BOUND).await;
        drop(sent);
        assert_eq!(
            breaker.read(),
            BreakerState::Open {
                retry_after: OPEN_FOR
            },
            "dropped at its bound's end, it failed"
        );

        tokio::time::advance(OPEN_FOR).await;
        drop(probe(&breaker).send(BOUND, until()));
        assert_eq!(
            breaker.read(),
            BreakerState::HalfOpen,
            "a probe dropped early hands its turn on"
        );
        probe(&breaker).send(BOUND, None).settle(None);
        let sent = probe(&breaker).send(BOUND, until());
        tokio::time::advance(BOUND).await;
        drop(sent);
        assert_eq!(
            breaker.read(),
            BreakerState::Open {
                retry_after: OPEN_FOR
            },
            "a probe dropped at its bound's end failed, one success of two in its series"
        );
        assert_eq!(
            said(&lines),
            [
                "open open_ms=30000 failed=1 window=1",
                "half_open",
                "open",
                "half_open",
                "open open_ms=30000 failed=1 window=1",
            ]
        );
    }

    /// An attempt ignored counts in neither direction, whenever it answered: a
    /// closed breaker's window holds nothing of it, the failure before it
    /// pushed out by none, and at the instant its bound ends at it has not
    /// timed out. Nothing is filed.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_ignored_counts_in_neither_direction_whenever_it_answered() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(2, 2);
        let one_failure = BreakerStats {
            outcomes: 1,
            failures: 1,
        };
        attempt(&breaker, true);
        for _ in 0..3 {
            let sent = breaker.admit(BOUND).expect("closed").send(BOUND, None);
            sent.ignore();
        }
        assert_eq!(breaker.stats(), one_failure);

        let until = Some(Instant::now() + BOUND);
        let sent = breaker.admit(BOUND).expect("closed").send(BOUND, until);
        tokio::time::advance(BOUND).await;
        sent.ignore();
        assert_eq!(
            breaker.stats(),
            one_failure,
            "ignored at its bound's end, it has not timed out"
        );
        assert!(states(&lines).is_empty(), "{:?}", states(&lines));

        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "open",
            "the second failure of a window that forgot none"
        );
    }

    /// A probe ignored hands its turn to the next call and moves nothing: the
    /// breaker stays half-open and its series keeps its count, neither
    /// advanced nor started over, whether a probe of it succeeded yet or none
    /// — no call refused, no line filed, no transition told, by the answer or
    /// by the probe sent after it. At the instant its bound ends at it has
    /// not failed.
    #[tokio::test(start_paused = true)]
    async fn a_probe_ignored_hands_its_turn_on_and_moves_nothing_whatever_its_series_holds() {
        use BreakerPhase::{Closed, HalfOpen, Open};

        let teller = Arc::new(Teller::new("upstream"));
        let breaker = Breaker::new(
            BreakerSettings {
                window: 1,
                failures: 1,
                open_for: OPEN_FOR,
                age: None,
                probes: 2,
            },
            Arc::clone(&teller),
        );
        let lines = nest_rs_testing::LogCapture::install();
        let mut events = teller.subscribe();
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;

        probe(&breaker).send(BOUND, None).ignore();
        assert_eq!(
            (breaker.read(), breaker.state()),
            (BreakerState::HalfOpen, "half_open"),
            "the turn is free, the breaker probing still"
        );
        let pending = probe(&breaker);
        assert_eq!(
            breaker.state(),
            "half_open",
            "the turn taken, its call not sent: a series under way stays half-open"
        );
        drop(pending);
        let sent = probe(&breaker).send(BOUND, Some(Instant::now() + BOUND));
        tokio::time::advance(BOUND).await;
        sent.ignore();
        assert_eq!(
            breaker.read(),
            BreakerState::HalfOpen,
            "ignored at its bound's end, the probe has not failed"
        );

        probe(&breaker).send(BOUND, None).settle(None);
        probe(&breaker).send(BOUND, None).ignore();
        assert_eq!(breaker.read(), BreakerState::HalfOpen, "one of two, kept");
        probe(&breaker).send(BOUND, None).settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);
        assert_eq!(states(&lines), ["open", "half_open", "closed"]);
        assert_eq!(
            told(&mut events),
            [
                (Closed, Open, false),
                (Open, HalfOpen, false),
                (HalfOpen, Closed, false),
            ]
        );
    }

    /// A probe its caller drops once sent, no probe of its series having
    /// succeeded, files the breaker open again, and the next probe half-open,
    /// after a probe ignored as after none.
    #[tokio::test(start_paused = true)]
    async fn a_probe_dropped_after_one_ignored_files_the_breaker_open_again() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = probing(2);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).ignore();
        drop(probe(&breaker).send(BOUND, None));
        assert_eq!(breaker.state(), "open");
        probe(&breaker).send(BOUND, None).settle(None);
        probe(&breaker).send(BOUND, None).settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);
        assert_eq!(
            states(&lines),
            ["open", "half_open", "open", "half_open", "closed"]
        );
    }

    /// A probe ignored after a switch hands on no turn of the breaker's since:
    /// the probe holding it keeps it.
    #[tokio::test(start_paused = true)]
    async fn a_probe_ignored_after_a_switch_takes_nothing_from_the_probe_after_it() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let stale = probe(&breaker).send(BOUND, None);
        let held_open = Duration::from_secs(10);
        breaker.open_manually(held_open);
        tokio::time::advance(held_open).await;
        let current = probe(&breaker).send(BOUND, None);
        stale.ignore();
        assert_eq!(
            refused_for(&breaker),
            BOUND,
            "the call out is still the probe"
        );
        current.settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);
    }

    /// The breaker opened by its failures, then closed again by its probe.
    async fn cycled(breaker: &Breaker) {
        while breaker.read() == BreakerState::Closed {
            attempt(breaker, true);
        }
        tokio::time::advance(OPEN_FOR).await;
        probe(breaker).send(BOUND, None).settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);
    }

    const ONE_FAILURE: BreakerStats = BreakerStats {
        outcomes: 1,
        failures: 1,
    };

    const NOTHING: BreakerStats = BreakerStats {
        outcomes: 0,
        failures: 0,
    };

    /// An answer counts only while the breaker has not opened since its
    /// attempt was admitted: sent before an opening and answering once the
    /// breaker closed again, a success pushes no failure out of the new
    /// window, and a failure opens nothing.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_sent_before_an_opening_counts_nowhere_once_the_breaker_closed_again() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(2, 2);
        let sent = || breaker.admit(BOUND).expect("closed").send(BOUND, None);
        let (success, another, failure) = (sent(), sent(), sent());
        cycled(&breaker).await;
        attempt(&breaker, true);

        success.settle(None);
        another.settle(None);
        assert_eq!(
            breaker.stats(),
            ONE_FAILURE,
            "no success from before the opening pushed the failure out"
        );
        failure.settle(Some(DOWN));
        assert_eq!(
            (breaker.read(), breaker.stats()),
            (BreakerState::Closed, ONE_FAILURE),
            "no failure from before the opening is the window's second"
        );
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "open",
            "the second failure since it closed"
        );
        assert_eq!(states(&lines), ["open", "half_open", "closed", "open"]);
    }

    /// However an attempt admitted before an opening ends once the breaker
    /// closed again, the new window holds nothing of it: dropped past its
    /// bound's end, where it had timed out; ignored; dropped with no bound;
    /// and its pass, kept unsent, sent only then and answering a failure.
    #[tokio::test(start_paused = true)]
    async fn however_an_attempt_from_before_an_opening_ends_the_new_window_holds_nothing_of_it() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(1, 1);
        let admitted = || breaker.admit(BOUND).expect("closed");
        let until = Some(Instant::now() + BOUND);
        let cut = admitted().send(BOUND, until);
        let ignored = admitted().send(BOUND, until);
        let dropped = admitted().send(BOUND, None);
        let unsent = admitted();
        cycled(&breaker).await;

        drop(cut);
        ignored.ignore();
        drop(dropped);
        assert!(!unsent.probes());
        unsent.postpone(Instant::now() + BOUND, BOUND);
        unsent.send(BOUND, None).settle(Some(DOWN));
        assert_eq!(
            (breaker.read(), breaker.stats()),
            (BreakerState::Closed, NOTHING),
            "a breaker one failure opens, and none of them was one"
        );
        assert_eq!(states(&lines), ["open", "half_open", "closed"]);
    }

    /// An attempt held past its run across an opening and the closing after
    /// it counts nowhere once it is taken up again, as one its run settles: a
    /// success pushes no failure out, and a failure opens nothing.
    #[cfg(feature = "reqwest")]
    #[tokio::test(start_paused = true)]
    async fn an_attempt_held_across_an_opening_counts_nowhere_once_the_breaker_closed_again() {
        let breaker = breaker(2, 2);
        let held = || {
            breaker
                .admit(BOUND)
                .expect("closed")
                .send(BOUND, None)
                .hold(BOUND)
        };
        let (success, another, failure) = (held(), held(), held());
        cycled(&breaker).await;
        attempt(&breaker, true);

        breaker.resume(success).settle(None);
        breaker.resume(another).settle(None);
        assert_eq!(breaker.stats(), ONE_FAILURE);
        breaker.resume(failure).settle(Some(DOWN));
        assert_eq!(
            (breaker.read(), breaker.stats()),
            (BreakerState::Closed, ONE_FAILURE)
        );
    }

    /// A probe is admitted in the opening it probes, and an attempt after it
    /// in the closing it made: across two openings each still counts — a
    /// failed probe opens the breaker again, the next closes it, and the
    /// failures answered since open it a third time.
    #[tokio::test(start_paused = true)]
    async fn a_probe_and_the_attempts_after_it_count_in_the_opening_they_were_admitted_in() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(2, 2);
        attempt(&breaker, true);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(Some(DOWN));
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(None);
        let late = breaker.admit(BOUND).expect("closed").send(BOUND, None);
        attempt(&breaker, true);
        late.settle(Some(DOWN));
        assert_eq!(
            states(&lines),
            ["open", "half_open", "open", "half_open", "closed", "open"]
        );
    }

    /// The state read is what a call arriving now meets: closed, open with what
    /// is left of the open time, never zero, then half-open whether the
    /// probe's turn is free, taken by a call not sent yet — which the lines
    /// still say is open — or out.
    #[tokio::test(start_paused = true)]
    async fn the_state_read_is_what_a_call_arriving_now_meets() {
        let breaker = breaker(1, 1);
        assert_eq!(breaker.read(), BreakerState::Closed);
        attempt(&breaker, true);
        assert_eq!(
            breaker.read(),
            BreakerState::Open {
                retry_after: OPEN_FOR
            }
        );
        tokio::time::advance(OPEN_FOR - Duration::from_millis(1)).await;
        assert_eq!(
            breaker.read(),
            BreakerState::Open {
                retry_after: Duration::from_millis(1)
            }
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        assert_eq!(breaker.read(), BreakerState::HalfOpen, "the turn is free");
        let pending = probe(&breaker);
        assert_eq!(
            (breaker.read(), breaker.state(), refused_for(&breaker)),
            (BreakerState::HalfOpen, "open", BOUND),
            "the turn taken, its call not sent: no time read, the probe's bound told",
        );
        let sent = pending.send(BOUND, None);
        assert_eq!(breaker.read(), BreakerState::HalfOpen, "the call out");
        sent.settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);
    }

    /// A manual opening refuses every call for its time, then lets one probe
    /// through as after any opening.
    #[tokio::test(start_paused = true)]
    async fn a_manual_opening_refuses_every_call_for_its_time_then_probes() {
        let breaker = breaker(4, 2);
        let held_open = Duration::from_secs(10);
        breaker.open_manually(held_open);
        assert_eq!(refused_for(&breaker), held_open);
        tokio::time::advance(held_open).await;
        probe(&breaker).send(BOUND, None).settle(None);
        assert_eq!(breaker.state(), "closed");
    }

    /// A manual opening of a breaker failures opened replaces its open time,
    /// counted from the call, shorter or longer: the switch does what it says.
    #[tokio::test(start_paused = true)]
    async fn a_manual_opening_of_an_open_breaker_replaces_its_open_time_from_the_call() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(Duration::from_secs(10)).await;
        breaker.open_manually(Duration::from_secs(5));
        assert_eq!(
            refused_for(&breaker),
            Duration::from_secs(5),
            "shorter than the 20 s left"
        );
        breaker.open_manually(Duration::from_secs(60));
        assert_eq!(refused_for(&breaker), Duration::from_secs(60), "longer");
    }

    /// A manual closing closes the breaker whatever it was in, and forgets its
    /// window: a failure counted before it adds to none after it.
    #[tokio::test(start_paused = true)]
    async fn a_manual_closing_closes_the_breaker_and_forgets_its_window() {
        let breaker = breaker(4, 2);
        attempt(&breaker, true);
        breaker.close_manually();
        attempt(&breaker, true);
        assert_eq!(
            breaker.state(),
            "closed",
            "the failure before it is forgotten"
        );
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open");
        breaker.close_manually();
        assert_eq!(breaker.read(), BreakerState::Closed);
    }

    /// A probe admitted or sent before a manual opening changes nothing when
    /// it answers a success, nor when it sends: the breaker stays open for the
    /// time it was given, and the probe after it, its own, alone closes it.
    #[tokio::test(start_paused = true)]
    async fn a_probe_admitted_or_sent_before_a_manual_opening_leaves_the_breaker_open_for_it() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let held_open = Duration::from_secs(10);

        let sent = probe(&breaker).send(BOUND, None);
        breaker.open_manually(held_open);
        tokio::time::advance(held_open).await;
        let current = probe(&breaker).send(BOUND, None);
        sent.settle(None);
        assert_eq!(
            breaker.read(),
            BreakerState::HalfOpen,
            "the probe sent before the switch closed nothing"
        );
        current.settle(Some(DOWN));
        assert_eq!(refused_for(&breaker), OPEN_FOR, "the probe after it failed");

        tokio::time::advance(OPEN_FOR).await;
        let admitted = probe(&breaker);
        breaker.open_manually(held_open);
        assert!(!admitted.probes(), "the switch took its turn");
        tokio::time::advance(held_open).await;
        let current = probe(&breaker);
        admitted.send(BOUND, None).settle(None);
        assert_eq!(
            states(&lines),
            ["open", "half_open", "open", "half_open", "open", "open"],
            "the probe admitted before the switch filed no half_open when it sent"
        );
        assert!(
            current.probes(),
            "the probe admitted before the switch took nothing"
        );
        assert_eq!(breaker.read(), BreakerState::HalfOpen);
        current.send(BOUND, None).settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);
        assert_eq!(
            states(&lines),
            [
                "open",
                "half_open",
                "open",
                "half_open",
                "open",
                "open",
                "half_open",
                "closed"
            ],
            "the probe after the switch filed its own half_open when it sent"
        );
    }

    /// An attempt held past its run counts when it is taken up again and
    /// settled, never before: a failure then opens the breaker, saying what it
    /// failed on.
    #[cfg(feature = "reqwest")]
    #[tokio::test(start_paused = true)]
    async fn an_attempt_held_past_its_run_counts_once_it_is_taken_up_again_and_settled() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(2, 2);
        let admitted = || breaker.admit(BOUND).expect("closed");
        let first = admitted().send(BOUND, None).hold(BOUND);
        let second = admitted()
            .send(BOUND, Some(Instant::now() + BOUND))
            .hold(BOUND);
        tokio::time::advance(2 * BOUND).await;
        assert_eq!(
            breaker.read(),
            BreakerState::Closed,
            "held past its bound's end, it has not timed out: its holder says when it has"
        );

        breaker.resume(first).settle(Some(DOWN));
        assert_eq!(breaker.read(), BreakerState::Closed, "one failure of two");
        breaker.resume(second).settle(Some(DOWN));
        assert_eq!(
            breaker.read(),
            BreakerState::Open {
                retry_after: OPEN_FOR
            }
        );
        let opened = lines.find(TARGET, MOVED);
        assert_eq!(opened.len(), 1, "{opened:?}");
        assert_eq!(opened[0].field("error").as_deref(), Some(DOWN));
    }

    /// A probe held past its run keeps its turn until it is taken up again,
    /// then hands it on exactly once: by its answer, closing the breaker or
    /// opening it again, or dropped with no answer, to the next call.
    #[cfg(feature = "reqwest")]
    #[tokio::test(start_paused = true)]
    async fn a_probe_held_past_its_run_keeps_its_turn_until_it_is_taken_up_again() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;

        let held = probe(&breaker).send(BOUND, None).hold(BOUND);
        tokio::time::advance(2 * BOUND).await;
        assert_eq!(breaker.read(), BreakerState::HalfOpen);
        assert_eq!(refused_for(&breaker), BOUND, "the turn is still held");
        drop(breaker.resume(held));
        assert_eq!(states(&lines), ["open", "half_open", "open"]);

        let held = probe(&breaker).send(BOUND, None).hold(BOUND);
        breaker.resume(held).settle(Some(DOWN));
        assert_eq!(refused_for(&breaker), OPEN_FOR, "the probe failed");

        tokio::time::advance(OPEN_FOR).await;
        let held = probe(&breaker).send(BOUND, None).hold(BOUND);
        assert_eq!(breaker.read(), BreakerState::HalfOpen);
        breaker.resume(held).settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);
        attempt(&breaker, false);
        assert_eq!(
            states(&lines),
            [
                "open",
                "half_open",
                "open",
                "half_open",
                "open",
                "half_open",
                "closed"
            ]
        );
    }

    /// A call refused beside a probe is told what is left of that probe's
    /// bound while its timer runs, and that whole bound once its answer is
    /// held past its run, at once and however long after: nothing gives a
    /// held probe up, so it is never told the tick of one being given up.
    #[cfg(feature = "reqwest")]
    #[tokio::test(start_paused = true)]
    async fn a_call_refused_beside_a_held_probe_is_told_that_probes_bound() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let second = Duration::from_secs(1);

        let sent = probe(&breaker).send(BOUND, None);
        tokio::time::advance(second).await;
        assert_eq!(refused_for(&breaker), BOUND - second, "sent, unanswered");
        let held = sent.hold(BOUND);
        assert_eq!(refused_for(&breaker), BOUND, "held as its head came");
        for _ in 0..3 {
            tokio::time::advance(4 * BOUND).await;
            assert_eq!(refused_for(&breaker), BOUND, "held, long past its bound");
            assert_eq!(breaker.refusing(), Some(BOUND));
        }
        breaker.resume(held).settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);

        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let stale = probe(&breaker).send(BOUND, None);
        breaker.open_manually(second);
        tokio::time::advance(second).await;
        let current = probe(&breaker).send(BOUND, None);
        let stale = stale.hold(BOUND);
        assert_eq!(
            refused_for(&breaker),
            BOUND,
            "a probe held from before a switch says nothing of the one sent since"
        );
        tokio::time::advance(BOUND - second).await;
        assert_eq!(refused_for(&breaker), second);
        drop(breaker.resume(stale));
        current.settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);
    }

    /// A manual switch while an attempt is held past its run leaves it nothing
    /// to change once it is taken up again: a failure opens nothing after a
    /// manual closing, and a probe's success closes nothing after a manual
    /// opening, nor takes the turn of the probe after it.
    #[cfg(feature = "reqwest")]
    #[tokio::test(start_paused = true)]
    async fn an_attempt_held_across_a_manual_switch_changes_nothing() {
        let breaker = breaker(1, 1);
        let held = breaker
            .admit(BOUND)
            .expect("closed")
            .send(BOUND, None)
            .hold(BOUND);
        breaker.close_manually();
        breaker.resume(held).settle(Some(DOWN));
        assert_eq!(breaker.read(), BreakerState::Closed);

        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let held = probe(&breaker).send(BOUND, None).hold(BOUND);
        let held_open = Duration::from_secs(10);
        breaker.open_manually(held_open);
        tokio::time::advance(held_open).await;
        let current = probe(&breaker).send(BOUND, None);
        breaker.resume(held).settle(None);
        assert_eq!(
            breaker.read(),
            BreakerState::HalfOpen,
            "the probe held across the switch closed nothing"
        );
        current.settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);
    }

    /// A probe answering a failure after a manual closing leaves the breaker
    /// closed, and a probe of the breaker opened since as it was.
    #[tokio::test(start_paused = true)]
    async fn a_probe_answering_a_failure_after_a_manual_closing_leaves_the_breaker_closed() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let sent = probe(&breaker).send(BOUND, None);
        breaker.close_manually();
        sent.settle(Some(DOWN));
        assert_eq!(breaker.read(), BreakerState::Closed);

        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let sent = probe(&breaker).send(BOUND, None);
        breaker.close_manually();
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let current = probe(&breaker).send(BOUND, None);
        sent.settle(Some(DOWN));
        assert_eq!(
            breaker.read(),
            BreakerState::HalfOpen,
            "the probe from before the switch opened nothing"
        );
        current.settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);
    }

    /// An attempt admitted before a manual closing counts nothing in the window
    /// the switch forgot.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_admitted_before_a_manual_closing_counts_nothing_in_the_window_it_forgot() {
        let breaker = breaker(2, 2);
        attempt(&breaker, true);
        let late = breaker.admit(BOUND).expect("closed").send(BOUND, None);
        breaker.close_manually();
        late.settle(Some(DOWN));
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "closed", "one failure since the switch");
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open");
    }

    /// A probe dropped after a switch, its call sent or not, hands on no turn
    /// of the breaker's since: the probe holding it keeps it.
    #[tokio::test(start_paused = true)]
    async fn a_probe_dropped_after_a_switch_takes_nothing_from_the_probe_after_it() {
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let admitted = probe(&breaker);
        let held_open = Duration::from_secs(10);
        breaker.open_manually(held_open);
        tokio::time::advance(held_open).await;
        let current = probe(&breaker);
        drop(admitted);
        assert_eq!(refused_for(&breaker), BOUND, "the turn is still taken");
        let sent = current.send(BOUND, None);
        breaker.open_manually(held_open);
        tokio::time::advance(held_open).await;
        let after = probe(&breaker).send(BOUND, None);
        drop(sent);
        assert_eq!(
            refused_for(&breaker),
            BOUND,
            "the call out is still the probe"
        );
        after.settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed);
    }

    /// A probe admitted before a switch, postponing or sending its call once
    /// the probe after it took the turn, changes nothing of that turn: a call
    /// refused beside it is told the bound of the probe holding it, no
    /// `half_open` is filed, and the turn passes on when that probe is
    /// dropped unsent.
    #[tokio::test(start_paused = true)]
    async fn a_probe_postponed_or_sent_after_a_switch_takes_nothing_from_the_probe_after_it() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(1, 1);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        let stale = probe(&breaker);
        let held_open = Duration::from_secs(10);
        breaker.open_manually(held_open);
        tokio::time::advance(held_open).await;
        let current = probe(&breaker);
        stale.postpone(Instant::now() + 2 * BOUND, BOUND);
        assert_eq!(
            refused_for(&breaker),
            BOUND,
            "the bound of the probe holding the turn"
        );
        let stale = stale.send(BOUND, None);
        assert_eq!(
            breaker.state(),
            "open",
            "the probe holding the turn has not sent its call"
        );
        drop(current);
        stale.settle(None);
        probe(&breaker).send(BOUND, None).settle(None);
        assert_eq!(breaker.read(), BreakerState::Closed, "the turn passed on");
        assert_eq!(states(&lines), ["open", "open", "half_open", "closed"]);
    }

    /// A manual switch files the transition's `warn` and counts as one, marked
    /// manual — an opening with the time it was given — each time, whether the
    /// state it enters is the one it finds or not; no transition of the
    /// breaker's own carries the mark: failures opening it, a probe sent, a
    /// probe abandoned once sent, a failed probe, or the probe closing it.
    #[tokio::test(start_paused = true)]
    async fn a_manual_switch_is_the_transitions_warn_marked_manual() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(1, 1);
        breaker.close_manually();
        breaker.open_manually(Duration::from_secs(45));
        breaker.open_manually(Duration::from_secs(45));
        breaker.close_manually();
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        drop(probe(&breaker).send(BOUND, None));
        probe(&breaker).send(BOUND, None).settle(Some(DOWN));
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(None);
        assert_eq!(
            said(&lines),
            [
                "closed manual",
                "open manual open_ms=45000",
                "open manual open_ms=45000",
                "closed manual",
                "open open_ms=30000 failed=1 window=1",
                "half_open",
                "open",
                "half_open",
                "open open_ms=30000 failed=1 window=1",
                "half_open",
                "closed",
            ]
        );
    }

    /// Each transition filed is told, leaving the phase the one before it
    /// entered: a probe sent, abandoned, the last of its series closing, and a
    /// switch by hand. What files no line tells nothing — a probe's turn given
    /// up unsent, a probe passing with more of its series to come.
    #[tokio::test(start_paused = true)]
    async fn each_transition_is_told_leaving_the_phase_the_one_before_it_entered() {
        use BreakerPhase::{Closed, HalfOpen, Open};

        let teller = Arc::new(Teller::new("upstream"));
        let breaker = Breaker::new(
            BreakerSettings {
                window: 1,
                failures: 1,
                open_for: OPEN_FOR,
                age: None,
                probes: 2,
            },
            Arc::clone(&teller),
        );
        let lines = nest_rs_testing::LogCapture::install();
        let mut events = teller.subscribe();

        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        drop(probe(&breaker));
        drop(probe(&breaker).send(BOUND, None));
        probe(&breaker).send(BOUND, None).settle(None);
        probe(&breaker).send(BOUND, None).settle(None);
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(None);
        breaker.open_manually(Duration::from_secs(10));
        breaker.close_manually();

        let told = told(&mut events);
        assert_eq!(
            told,
            [
                (Closed, Open, false),
                (Open, HalfOpen, false),
                (HalfOpen, Open, false),
                (Open, HalfOpen, false),
                (HalfOpen, Closed, false),
                (Closed, Open, false),
                (Open, HalfOpen, false),
                (HalfOpen, Open, true),
                (Open, Closed, true),
            ]
        );
        assert_eq!(
            states(&lines),
            told.iter()
                .map(|(_, to, _)| to.as_str())
                .collect::<Vec<_>>(),
            "one event a line, entering the state the line says"
        );
    }

    /// A manual switch counts by its instance and the state it entered, marked
    /// manual; no transition of the breaker's own carries the mark.
    #[cfg(feature = "opentelemetry")]
    #[tokio::test(start_paused = true)]
    async fn a_manual_switch_is_counted_by_the_state_entered_marked_manual() {
        use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
        use opentelemetry_sdk::metrics::{
            InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        };

        use crate::counters::{INSTANCE_ATTRIBUTE, MANUAL_ATTRIBUTE, STATE_ATTRIBUTE, TRANSITIONS};

        let breaker = breaker(1, 1);
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());

        breaker.open_manually(Duration::from_secs(10));
        breaker.close_manually();
        attempt(&breaker, true);
        tokio::time::advance(OPEN_FOR).await;
        drop(probe(&breaker).send(BOUND, None));
        probe(&breaker).send(BOUND, None).settle(Some(DOWN));
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(None);
        provider.force_flush().expect("the export");

        let exported = exporter
            .get_finished_metrics()
            .expect("the exported metrics");
        let mut counted: Vec<(String, String, u64)> = exported
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .filter(|metric| metric.name() == TRANSITIONS)
            .flat_map(|metric| {
                let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                    panic!("a counter: {metric:?}");
                };
                sum.data_points()
                    .map(|point| {
                        let attribute = |key: &str| {
                            point
                                .attributes()
                                .find(|kv| kv.key.as_str() == key)
                                .map(|kv| kv.value.to_string())
                                .unwrap_or_default()
                        };
                        assert_eq!(
                            attribute(INSTANCE_ATTRIBUTE),
                            "upstream",
                            "every point names its policy, a manual one too"
                        );
                        (
                            attribute(STATE_ATTRIBUTE),
                            attribute(MANUAL_ATTRIBUTE),
                            point.value(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        counted.sort();
        assert_eq!(
            counted,
            [
                ("closed".to_owned(), String::new(), 1),
                ("closed".to_owned(), "true".to_owned(), 1),
                ("half_open".to_owned(), String::new(), 3),
                ("open".to_owned(), String::new(), 3),
                ("open".to_owned(), "true".to_owned(), 1),
            ],
            "opened and closed by hand, then by the breaker: a failure, a probe abandoned, a \
             failed probe and the probe closing it"
        );
    }

    /// What a `tracing` subscriber does at a transition's line, beside taking
    /// it down: nothing, or switch the breaker by hand from inside the line.
    #[derive(Clone, Copy)]
    enum Inside {
        Nothing,
        Open,
        Close,
    }

    /// An app's `tracing` subscriber reading the breaker at each transition's
    /// line, and switching it by hand from inside the line when `inside` says
    /// so, one line an entry.
    struct Reading {
        breaker: Arc<Breaker>,
        inside: Mutex<VecDeque<Inside>>,
        /// Each line, as `state`, then `manual` where it is marked so.
        said: Arc<Mutex<Vec<String>>>,
        /// How many lines were filed with the breaker's lock held.
        locked: Arc<AtomicUsize>,
    }

    impl Reading {
        fn new(breaker: &Arc<Breaker>) -> Self {
            Self {
                breaker: Arc::clone(breaker),
                inside: Mutex::default(),
                said: Arc::default(),
                locked: Arc::default(),
            }
        }
    }

    /// A transition line's `state`, its `open_ms`, and whether it is marked
    /// `manual`.
    #[derive(Default)]
    struct Line {
        message: String,
        state: String,
        open_ms: Option<u64>,
        manual: bool,
    }

    impl Line {
        /// The transition's line `event` is, when it is one.
        fn of(event: &tracing::Event<'_>) -> Option<Self> {
            let mut line = Self::default();
            event.record(&mut line);
            (line.message == MOVED).then_some(line)
        }

        fn said(&self) -> String {
            let manual = if self.manual { " manual" } else { "" };
            format!("{}{manual}", self.state)
        }
    }

    impl tracing::field::Visit for Line {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            if field.name() == "state" {
                value.clone_into(&mut self.state);
            }
        }

        fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
            if field.name() == "manual" {
                self.manual = value;
            }
        }

        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            if field.name() == "open_ms" {
                self.open_ms = Some(value);
            }
        }

        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.message = format!("{value:?}");
            }
        }
    }

    /// What a subscriber of these tests answers for all but an event.
    macro_rules! taking_events_alone {
        () => {
            fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
                true
            }

            fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                tracing::span::Id::from_u64(1)
            }

            fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

            fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

            fn enter(&self, _: &tracing::span::Id) {}

            fn exit(&self, _: &tracing::span::Id) {}
        };
    }

    impl tracing::Subscriber for Reading {
        taking_events_alone!();

        fn event(&self, event: &tracing::Event<'_>) {
            let Some(line) = Line::of(event) else {
                return;
            };
            self.said.lock().push(line.said());
            // Read or switched under its lock, the breaker would wait for itself.
            if self.breaker.locked() {
                self.locked.fetch_add(1, Ordering::SeqCst);
                return;
            }
            let _read = (self.breaker.read(), self.breaker.stats());
            let inside = self.inside.lock().pop_front();
            match inside.unwrap_or(Inside::Nothing) {
                Inside::Nothing => {}
                Inside::Open => self.breaker.open_manually(OPEN_FOR),
                Inside::Close => self.breaker.close_manually(),
            }
        }
    }

    /// A transition is told once the breaker's lock is let go, so an app's
    /// `tracing` subscriber reads the breaker and switches it from inside a
    /// line. Whatever moved the breaker — failures, a probe sent, abandoned,
    /// failed or closing it, a switch by hand — and whatever a line switched
    /// from inside, one switching again from inside its own line too, the
    /// lines and the events say the transitions in the order they took effect,
    /// and once the call returns the last line says the state the breaker is
    /// in.
    #[tokio::test(start_paused = true)]
    async fn a_line_is_filed_with_the_breaker_free_and_in_the_order_of_the_transitions() {
        use BreakerPhase::{Closed, HalfOpen, Open};

        let teller = Arc::new(Teller::new("upstream"));
        let breaker = Arc::new(Breaker::new(
            BreakerSettings {
                window: 1,
                failures: 1,
                open_for: OPEN_FOR,
                age: None,
                probes: 1,
            },
            Arc::clone(&teller),
        ));
        let mut events = teller.subscribe();
        let reading = Reading::new(&breaker);
        let (said, locked) = (Arc::clone(&reading.said), Arc::clone(&reading.locked));
        let reading = Arc::new(reading);
        let _reading = tracing::subscriber::set_default(Arc::clone(&reading));
        let inside = |switches: &[Inside]| reading.inside.lock().extend(switches);
        let last_said = || {
            let said = said.lock();
            let last = said.last().map_or("", String::as_str);
            last.split(' ').next().unwrap_or_default().to_owned()
        };

        attempt(&breaker, true);
        assert_eq!(last_said(), breaker.state(), "failures opened it");
        tokio::time::advance(OPEN_FOR).await;
        let sent = probe(&breaker).send(BOUND, None);
        assert_eq!(last_said(), breaker.state(), "a probe sent");
        drop(sent);
        assert_eq!(last_said(), breaker.state(), "the probe abandoned");
        probe(&breaker).send(BOUND, None).settle(Some(DOWN));
        assert_eq!(last_said(), breaker.state(), "a probe failed");
        tokio::time::advance(OPEN_FOR).await;
        probe(&breaker).send(BOUND, None).settle(None);
        assert_eq!(last_said(), breaker.state(), "the probe closing it");

        inside(&[Inside::Close]);
        attempt(&breaker, true);
        assert_eq!(
            (last_said().as_str(), breaker.state()),
            ("closed", "closed"),
            "closed from inside the line of its opening"
        );
        inside(&[Inside::Close, Inside::Open]);
        breaker.open_manually(OPEN_FOR);
        assert_eq!(
            (last_said().as_str(), breaker.state()),
            ("open", "open"),
            "opened by hand, closed from inside that line, and opened from inside the closing's"
        );
        tokio::time::advance(OPEN_FOR).await;
        inside(&[Inside::Close]);
        let sent = probe(&breaker).send(BOUND, None);
        assert_eq!(
            (last_said().as_str(), breaker.state()),
            ("closed", "closed"),
            "closed from inside the line of its probe"
        );
        sent.settle(Some(DOWN));
        breaker.close_manually();

        assert_eq!(
            *said.lock(),
            [
                "open",
                "half_open",
                "open",
                "half_open",
                "open",
                "half_open",
                "closed",
                "open",
                "closed manual",
                "open manual",
                "closed manual",
                "open manual",
                "half_open",
                "closed manual",
                "closed manual",
            ],
            "each transition's line, a switch made inside a line after that line"
        );
        assert_eq!(locked.load(Ordering::SeqCst), 0, "no line under the lock");
        let told = told(&mut events);
        assert_eq!(
            told.iter()
                .map(|&(from, to, _)| (from, to))
                .collect::<Vec<_>>(),
            [
                (Closed, Open),
                (Open, HalfOpen),
                (HalfOpen, Open),
                (Open, HalfOpen),
                (HalfOpen, Open),
                (Open, HalfOpen),
                (HalfOpen, Closed),
                (Closed, Open),
                (Open, Closed),
                (Closed, Open),
                (Open, Closed),
                (Closed, Open),
                (Open, HalfOpen),
                (HalfOpen, Closed),
                (Closed, Closed),
            ],
            "one event a line, each leaving the phase the one before it entered"
        );
    }

    /// Whether every transition `breaker` made was told, and no thread holds
    /// the turn to tell them.
    fn all_told(breaker: &Breaker) -> bool {
        let held = breaker.published.lock();
        held.untold.is_empty() && !held.telling
    }

    /// A subscriber stopping the first transition's line it files until it is
    /// let go on, the thread filing it holding the turn to tell meanwhile.
    struct Stopped {
        said: Arc<Mutex<Vec<String>>>,
        stopped: AtomicBool,
        filing: std::sync::mpsc::Sender<()>,
        go_on: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl tracing::Subscriber for Stopped {
        taking_events_alone!();

        fn event(&self, event: &tracing::Event<'_>) {
            let Some(line) = Line::of(event) else {
                return;
            };
            self.said.lock().push(line.said());
            if !self.stopped.swap(true, Ordering::SeqCst) {
                self.filing.send(()).expect("the test waits for the line");
                self.go_on.lock().recv().expect("the test lets it go on");
            }
        }
    }

    /// A transition made while another thread tells one is left to that
    /// thread: its own returns at once, the breaker moved and nothing of it
    /// told yet, and the thread telling says it after its own, line and event,
    /// before it gives the turn up.
    #[test]
    fn a_transition_made_while_another_thread_tells_is_told_by_that_thread_after_its_own() {
        use std::sync::mpsc::channel;

        use BreakerPhase::{Closed, Open};

        let teller = Arc::new(Teller::new("upstream"));
        let breaker = Arc::new(Breaker::new(
            BreakerSettings {
                window: 1,
                failures: 1,
                open_for: OPEN_FOR,
                age: None,
                probes: 1,
            },
            Arc::clone(&teller),
        ));
        let mut events = teller.subscribe();
        let said = Arc::new(Mutex::new(Vec::new()));
        let (filing, in_line) = channel();
        let (let_go, go_on) = channel();
        let lines = tracing::Dispatch::new(Stopped {
            said: Arc::clone(&said),
            stopped: AtomicBool::new(false),
            filing,
            go_on: Mutex::new(go_on),
        });

        let telling = {
            let (breaker, lines) = (Arc::clone(&breaker), lines.clone());
            std::thread::spawn(move || {
                let _lines = tracing::dispatcher::set_default(&lines);
                breaker.open_manually(OPEN_FOR);
            })
        };
        in_line.recv().expect("the opening's line is being filed");
        let (returned, closed) = channel();
        let closing = {
            let breaker = Arc::clone(&breaker);
            std::thread::spawn(move || {
                let _lines = tracing::dispatcher::set_default(&lines);
                breaker.close_manually();
                returned.send(()).expect("the test waits for the closing");
            })
        };
        if closed.recv_timeout(Duration::from_secs(5)).is_err() {
            // Joined, the two threads would hang the test past its verdict.
            std::mem::forget((telling, closing));
            panic!("the closing waits for the thread telling the opening");
        }
        assert_eq!(breaker.state(), "closed", "the closing took effect");
        assert_eq!(
            (said.lock().as_slice(), told(&mut events).len()),
            (["open manual".to_owned()].as_slice(), 0),
            "and is not told yet, the opening's line still being filed"
        );

        let_go.send(()).expect("the opening's line waits");
        telling.join().expect("the thread telling");
        closing.join().expect("the thread closing");
        assert_eq!(*said.lock(), ["open manual", "closed manual"]);
        assert_eq!(
            told(&mut events),
            [(Closed, Open, true), (Open, Closed, true)]
        );
        assert!(all_told(&breaker), "the turn given up, nothing left");
    }

    /// A subscriber taking down each transition's line as `state`, with its
    /// `open_ms` and the thread filing it, and counting the lines two threads
    /// filed at once. It stops
    /// the first line it files until `behind` threads have each made a
    /// transition behind it, so the thread filing it has theirs to tell.
    struct Ordered {
        said: Mutex<Vec<(String, Option<u64>, std::thread::ThreadId)>>,
        filing: AtomicBool,
        together: AtomicUsize,
        stopped: AtomicBool,
        behind: usize,
        /// The threads back from their first switch.
        back: AtomicUsize,
    }

    impl tracing::Subscriber for Ordered {
        taking_events_alone!();

        fn event(&self, event: &tracing::Event<'_>) {
            let Some(line) = Line::of(event) else {
                return;
            };
            if self.filing.swap(true, Ordering::SeqCst) {
                self.together.fetch_add(1, Ordering::SeqCst);
            }
            self.said
                .lock()
                .push((line.state, line.open_ms, std::thread::current().id()));
            if !self.stopped.swap(true, Ordering::SeqCst) {
                // Bounded, so threads that wait for this line fail the test
                // where they would hang it.
                let waited = std::time::Instant::now();
                while self.back.load(Ordering::SeqCst) < self.behind
                    && waited.elapsed() < Duration::from_secs(5)
                {
                    std::thread::yield_now();
                }
            }
            // Leaves the other threads time to queue behind this line.
            std::thread::yield_now();
            self.filing.store(false, Ordering::SeqCst);
        }
    }

    /// Threads switching one breaker at once, the first to file a line held
    /// in it until every other has made a transition behind it: each thread
    /// returns leaving its own to the one telling, which tells every one once,
    /// one thread at a time, each thread's in the order it made them and each
    /// event leaving the phase the one before it entered. At no instant is a
    /// transition queued with no thread telling, and once the threads have
    /// returned none is left, the last line saying the state the breaker is
    /// in.
    #[test]
    fn transitions_made_on_several_threads_are_each_told_once_and_in_order() {
        const THREADS: usize = 8;
        // Under the 256 events a subscription holds unread, the threads' together.
        const SWITCHES: usize = 30;

        let teller = Arc::new(Teller::new("upstream"));
        let breaker = Arc::new(Breaker::new(
            BreakerSettings {
                window: 1,
                failures: 1,
                open_for: OPEN_FOR,
                age: None,
                probes: 1,
            },
            Arc::clone(&teller),
        ));
        let mut events = teller.subscribe();
        let ordered = Arc::new(Ordered {
            said: Mutex::default(),
            filing: AtomicBool::new(false),
            together: AtomicUsize::new(0),
            stopped: AtomicBool::new(false),
            behind: THREADS - 1,
            back: AtomicUsize::new(0),
        });
        let lines = tracing::Dispatch::new(Arc::clone(&ordered));
        let start = Arc::new(std::sync::Barrier::new(THREADS));
        // An open time no other switch asks, later than every one its thread
        // asked before.
        let open_ms = |thread: usize, switch: usize| (1 + thread + THREADS * switch) as u64;

        let switching: Vec<_> = (0..THREADS)
            .map(|thread| {
                let (breaker, lines, start, ordered) = (
                    Arc::clone(&breaker),
                    lines.clone(),
                    Arc::clone(&start),
                    Arc::clone(&ordered),
                );
                std::thread::spawn(move || {
                    let _lines = tracing::dispatcher::set_default(&lines);
                    start.wait();
                    for switch in 0..SWITCHES {
                        if switch % 2 == 0 {
                            breaker.open_manually(Duration::from_millis(open_ms(thread, switch)));
                        } else {
                            breaker.close_manually();
                        }
                        if switch == 0 {
                            ordered.back.fetch_add(1, Ordering::SeqCst);
                        }
                        std::thread::yield_now();
                    }
                })
            })
            .collect();
        let mut orphaned = 0;
        while !switching.iter().all(std::thread::JoinHandle::is_finished) {
            let held = breaker.published.lock();
            orphaned += usize::from(!held.telling && !held.untold.is_empty());
        }
        for thread in switching {
            thread.join().expect("a switching thread");
        }

        assert_eq!(orphaned, 0, "a transition queued, and no thread telling");
        assert!(all_told(&breaker), "none left untold, the turn given up");
        assert_eq!(
            ordered.together.load(Ordering::SeqCst),
            0,
            "one thread at a time"
        );
        let said = ordered.said.lock();
        let told = told(&mut events);
        assert_eq!(
            (said.len(), told.len()),
            (THREADS * SWITCHES, THREADS * SWITCHES),
            "a line and an event a switch"
        );
        let teller = said[0].2;
        for thread in 0..THREADS {
            let first = Some(open_ms(thread, 0));
            let filed_by = said
                .iter()
                .find(|(_, open_ms, _)| *open_ms == first)
                .map(|&(.., by)| by);
            assert_eq!(
                filed_by,
                Some(teller),
                "a thread's first switch, told by the thread the first line held"
            );
        }
        let mut left = BreakerPhase::Closed;
        for ((state, ..), &(from, to, manual)) in said.iter().zip(&told) {
            assert_eq!(
                (from, to.as_str(), manual),
                (left, state.as_str(), true),
                "each leaves the phase the one before it entered, as its line says"
            );
            left = to;
        }
        assert_eq!(left.as_str(), breaker.state(), "the last one told");
        for thread in 0..THREADS {
            let opened: Vec<u64> = said
                .iter()
                .filter_map(|&(_, open_ms, _)| open_ms)
                .filter(|&open_ms| (open_ms as usize - 1) % THREADS == thread)
                .collect();
            let made: Vec<u64> = (0..SWITCHES)
                .step_by(2)
                .map(|switch| open_ms(thread, switch))
                .collect();
            assert_eq!(opened, made, "a thread's openings, each once and in order");
        }
        let closed = said.iter().filter(|(state, ..)| state == "closed").count();
        assert_eq!(closed, THREADS * SWITCHES / 2, "and every closing");
    }

    /// A subscriber keeping the state the last transition's line said, and
    /// counting the lines, and the ones two threads filed at once.
    #[derive(Default)]
    struct Last {
        state: Mutex<String>,
        filed: AtomicUsize,
        filing: AtomicBool,
        together: AtomicUsize,
    }

    impl tracing::Subscriber for Last {
        taking_events_alone!();

        fn event(&self, event: &tracing::Event<'_>) {
            let Some(line) = Line::of(event) else {
                return;
            };
            if self.filing.swap(true, Ordering::SeqCst) {
                self.together.fetch_add(1, Ordering::SeqCst);
            }
            *self.state.lock() = line.state;
            self.filed.fetch_add(1, Ordering::SeqCst);
            self.filing.store(false, Ordering::SeqCst);
        }
    }

    /// Threads switching one breaker at the same instant, round after round,
    /// half opening it and half closing it: a thread ending its turn to tell
    /// races the others queueing theirs. Once a round's threads are back,
    /// every transition of it is told — none waits for the next one made —
    /// and the last line says the state the breaker is in, so none was told
    /// before one made earlier; no two threads ever filed at once.
    #[test]
    fn switches_racing_round_after_round_leave_none_untold_and_the_last_line_true() {
        const THREADS: usize = 4;
        const ROUNDS: usize = 2000;
        const SWEEP: usize = 251;

        let breaker = Arc::new(breaker(1, 1));
        let last = Arc::new(Last::default());
        let lines = tracing::Dispatch::new(Arc::clone(&last));
        let (round, back) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let racing: Vec<_> = (0..THREADS)
            .map(|thread| {
                let (breaker, lines) = (Arc::clone(&breaker), lines.clone());
                let (round, back) = (Arc::clone(&round), Arc::clone(&back));
                std::thread::spawn(move || {
                    let _lines = tracing::dispatcher::set_default(&lines);
                    for next in 1..=ROUNDS {
                        while round.load(Ordering::SeqCst) < next {
                            std::thread::yield_now();
                        }
                        // Later by a time that sweeps, round after round,
                        // across the turn of the thread that tells first.
                        for _ in 0..(next * thread) % SWEEP {
                            std::hint::spin_loop();
                        }
                        if thread % 2 == 0 {
                            breaker.open_manually(OPEN_FOR);
                        } else {
                            breaker.close_manually();
                        }
                        back.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();

        let mut untold = Vec::new();
        let mut stale = Vec::new();
        for next in 1..=ROUNDS {
            round.store(next, Ordering::SeqCst);
            while back.load(Ordering::SeqCst) < next * THREADS {
                std::thread::yield_now();
            }
            if !all_told(&breaker) {
                untold.push(next);
            }
            if *last.state.lock() != breaker.state() {
                stale.push(next);
            }
        }
        for thread in racing {
            thread.join().expect("a racing thread");
        }

        assert_eq!(
            (untold, stale),
            (Vec::new(), Vec::new()),
            "the rounds that left a transition untold, and the ones whose last line was not the \
             breaker's state"
        );
        assert_eq!(
            (
                last.filed.load(Ordering::SeqCst),
                last.together.load(Ordering::SeqCst)
            ),
            (ROUNDS * THREADS, 0),
            "a line a switch, one thread at a time"
        );
    }

    /// A subscriber panicking out of the first transition's line it files,
    /// once it has closed the breaker from inside it.
    struct Panicking {
        breaker: Arc<Breaker>,
        said: Arc<Mutex<Vec<String>>>,
        panicked: AtomicBool,
    }

    impl tracing::Subscriber for Panicking {
        taking_events_alone!();

        fn event(&self, event: &tracing::Event<'_>) {
            let Some(line) = Line::of(event) else {
                return;
            };
            if !self.panicked.swap(true, Ordering::SeqCst) {
                // Switched under its lock, the breaker would wait for itself.
                if !self.breaker.locked() {
                    self.breaker.close_manually();
                }
                panic!("the subscriber fails on this line");
            }
            self.said.lock().push(line.said());
        }
    }

    /// A subscriber panicking out of a line gives the turn to tell back as it
    /// unwinds: the transitions queued behind that line are told with the next
    /// one made, in order, where a turn kept would tell no transition again.
    #[test]
    fn a_line_that_panics_leaves_the_transitions_behind_it_to_the_next_one_made() {
        let breaker = Arc::new(breaker(1, 1));
        let said = Arc::new(Mutex::new(Vec::new()));
        let _lines = tracing::subscriber::set_default(Panicking {
            breaker: Arc::clone(&breaker),
            said: Arc::clone(&said),
            panicked: AtomicBool::new(false),
        });

        let opening = std::panic::AssertUnwindSafe(|| breaker.open_manually(OPEN_FOR));
        assert!(std::panic::catch_unwind(opening).is_err());
        assert_eq!(breaker.state(), "closed", "closed from inside the line");
        assert!(
            said.lock().is_empty() && !breaker.published.lock().telling,
            "the closing waits, the turn free"
        );

        breaker.open_manually(OPEN_FOR);
        assert_eq!(*said.lock(), ["closed manual", "open manual"]);
        assert!(all_told(&breaker));
    }

    /// A subscriber panicking out of every transition's line.
    struct Failing;

    impl tracing::Subscriber for Failing {
        taking_events_alone!();

        fn event(&self, event: &tracing::Event<'_>) {
            assert!(
                Line::of(event).is_none(),
                "the subscriber fails on every transition's line"
            );
        }
    }

    /// Whether `moving` unwinds.
    fn unwinds(moving: impl FnOnce()) -> bool {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(moving)).is_err()
    }

    /// The breaker's lines failing no more, the calls from now on leave the
    /// state it is in: what is left of an open time waited, the next is let
    /// through, and its success closes the breaker, nothing left to tell.
    async fn a_later_call_closes(breaker: &Breaker) {
        if let Err(wait) = breaker.admit(BOUND).map(drop) {
            tokio::time::advance(wait).await;
        }
        breaker
            .admit(BOUND)
            .expect("the call is let through")
            .send(BOUND, None)
            .settle(None);
        assert_eq!(breaker.state(), "closed");
        assert!(all_told(breaker));
    }

    /// A breaker one failure opened, its open time passed: the next call probes.
    async fn probe_due(breaker: &Breaker) {
        attempt(breaker, true);
        tokio::time::advance(OPEN_FOR).await;
    }

    /// A line panicking out of the first probe's half-opening unwinds its
    /// send with the probe's turn handed on, the breaker open again for the
    /// next call to probe: left half-open on a probe nothing holds, it would
    /// refuse every call until a switch by hand. The line of that handing on
    /// panics too, from a destructor as the thread unwinds, and ends there.
    #[tokio::test(start_paused = true)]
    async fn a_line_panicking_as_the_first_probe_is_sent_hands_the_probes_turn_on() {
        let breaker = breaker(1, 1);
        probe_due(&breaker).await;
        let probe = breaker.admit(BOUND).expect("the open time passed");
        let lines = tracing::subscriber::set_default(Failing);
        assert!(unwinds(|| probe.send(BOUND, None).settle(None)));
        drop(lines);

        assert_eq!(breaker.state(), "open", "the probe's turn is free");
        assert!(all_told(&breaker), "neither line is told again");
        a_later_call_closes(&breaker).await;
    }

    /// A line panicking out of an opening leaves the breaker open for its
    /// open time, whatever opened it: the failures of its window, a failed
    /// probe, or a switch by hand.
    #[tokio::test(start_paused = true)]
    async fn a_line_panicking_out_of_an_opening_leaves_the_breaker_open_for_its_open_time() {
        let by_failures = breaker(1, 1);
        let by_a_probe = breaker(1, 1);
        probe_due(&by_a_probe).await;
        let probe = by_a_probe
            .admit(BOUND)
            .expect("the open time passed")
            .send(BOUND, None);
        let by_hand = breaker(1, 1);

        let lines = tracing::subscriber::set_default(Failing);
        assert!(unwinds(|| attempt(&by_failures, true)));
        assert!(unwinds(|| probe.settle(Some(DOWN))));
        assert!(unwinds(|| by_hand.open_manually(OPEN_FOR)));
        drop(lines);

        let opened = [by_failures, by_a_probe, by_hand];
        for breaker in &opened {
            assert_eq!((breaker.state(), refused_for(breaker)), ("open", OPEN_FOR));
        }
        for breaker in &opened {
            a_later_call_closes(breaker).await;
        }
    }

    /// A line panicking out of a closing leaves the breaker closed and
    /// counting: by the last probe of its series, or by hand.
    #[tokio::test(start_paused = true)]
    async fn a_line_panicking_out_of_a_closing_leaves_the_breaker_closed() {
        let by_a_probe = breaker(1, 1);
        probe_due(&by_a_probe).await;
        let probe = by_a_probe
            .admit(BOUND)
            .expect("the open time passed")
            .send(BOUND, None);
        let by_hand = breaker(1, 1);
        attempt(&by_hand, true);

        let lines = tracing::subscriber::set_default(Failing);
        assert!(unwinds(|| probe.settle(None)));
        assert!(unwinds(|| by_hand.close_manually()));
        drop(lines);

        for breaker in [by_a_probe, by_hand] {
            assert_eq!(breaker.state(), "closed");
            attempt(&breaker, false);
            assert_eq!(breaker.stats().outcomes, 1, "it counts again");
            assert!(all_told(&breaker));
        }
    }

    /// A probe abandoned once sent files the breaker open again from its
    /// drop: a line panicking out of it leaves the probe's turn free, as the
    /// drop of a probe never sent and an ignored answer do, which file
    /// nothing and so never unwind.
    #[tokio::test(start_paused = true)]
    async fn a_line_panicking_as_a_probes_turn_is_handed_on_leaves_the_turn_free() {
        let abandoned = breaker(1, 1);
        probe_due(&abandoned).await;
        let sent = abandoned
            .admit(BOUND)
            .expect("the open time passed")
            .send(BOUND, None);
        let withdrawn = breaker(1, 1);
        probe_due(&withdrawn).await;
        let pass = withdrawn.admit(BOUND).expect("the open time passed");
        let ignored = breaker(1, 1);
        probe_due(&ignored).await;
        let answered = ignored
            .admit(BOUND)
            .expect("the open time passed")
            .send(BOUND, None);

        let lines = tracing::subscriber::set_default(Failing);
        assert!(unwinds(|| drop(sent)), "the breaker says it is open again");
        assert!(!unwinds(|| drop(pass)));
        assert!(!unwinds(|| answered.ignore()));
        drop(lines);

        assert_eq!(
            (abandoned.state(), withdrawn.state(), ignored.state()),
            ("open", "open", "half_open")
        );
        for breaker in [abandoned, withdrawn, ignored] {
            assert!(breaker.admit(BOUND).is_ok(), "the next call probes at once");
            a_later_call_closes(&breaker).await;
        }
    }

    /// An attempt dropped as its caller's own panic unwinds tells its
    /// transition from a destructor: a line panicking there ends there, where
    /// a second panic out of a destructor would abort the process — a probe
    /// abandoned, and an attempt past its bound whose timeout opens the
    /// breaker.
    #[tokio::test(start_paused = true)]
    async fn a_line_panicking_as_an_attempt_is_dropped_by_a_panic_ends_in_its_drop() {
        let probed = breaker(1, 1);
        probe_due(&probed).await;
        let probe = probed
            .admit(BOUND)
            .expect("the open time passed")
            .send(BOUND, None);
        let timed_out = breaker(1, 1);
        let late = timed_out
            .admit(BOUND)
            .expect("a closed breaker admits")
            .send(BOUND, Some(Instant::now()));

        let lines = tracing::subscriber::set_default(Failing);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _dropped = (probe, late);
            panic!("the caller's own");
        }));
        drop(lines);

        let payload = unwound.expect_err("the caller's panic unwinds on");
        assert_eq!(
            payload.downcast_ref::<&str>(),
            Some(&"the caller's own"),
            "its own panic, and no line's"
        );
        assert_eq!((probed.state(), timed_out.state()), ("open", "open"));
        for breaker in [probed, timed_out] {
            a_later_call_closes(&breaker).await;
        }
    }

    /// A subscriber that has another thread close the breaker by hand from
    /// inside the first transition's line it files, and panics out of the
    /// next: that thread's transition, queued behind the line and told here.
    struct FailingOnAnothers {
        breaker: Arc<Breaker>,
        filed: AtomicUsize,
    }

    impl tracing::Subscriber for FailingOnAnothers {
        taking_events_alone!();

        fn event(&self, event: &tracing::Event<'_>) {
            if Line::of(event).is_none() {
                return;
            }
            assert_eq!(
                self.filed.fetch_add(1, Ordering::SeqCst),
                0,
                "the subscriber fails on the other thread's line"
            );
            let breaker = Arc::clone(&self.breaker);
            std::thread::spawn(move || breaker.close_manually())
                .join()
                .expect("the other thread queues its transition and returns");
        }
    }

    /// A line panicking on a transition another thread made, told by the
    /// thread sending a probe, unwinds that probe's send: the breaker is in
    /// the phase the other thread's switch put it in, which voided the
    /// probe's turn, and nothing is left to tell.
    #[tokio::test(start_paused = true)]
    async fn a_line_panicking_on_another_threads_transition_unwinds_the_thread_telling_it() {
        let breaker = Arc::new(breaker(1, 1));
        probe_due(&breaker).await;
        let probe = breaker.admit(BOUND).expect("the open time passed");
        let lines = tracing::subscriber::set_default(FailingOnAnothers {
            breaker: Arc::clone(&breaker),
            filed: AtomicUsize::new(0),
        });
        assert!(unwinds(|| probe.send(BOUND, None).settle(None)));
        drop(lines);

        assert_eq!(breaker.state(), "closed", "closed by the other thread");
        assert!(all_told(&breaker));
        attempt(&breaker, false);
        assert_eq!(breaker.stats().outcomes, 1, "it counts again");
    }

    /// A reader's waker panicking as it is woken, as an executor's own may.
    struct PanickingWaker;

    impl std::task::Wake for PanickingWaker {
        fn wake(self: Arc<Self>) {
            panic!("the reader's waker fails");
        }
    }

    /// A subscription's waker panicking out of a transition's event unwinds
    /// what moved the breaker, as a line does: the breaker is in the phase it
    /// entered, the event is the subscription's to read, and the probe whose
    /// send it unwound hands its turn on.
    #[tokio::test(start_paused = true)]
    async fn a_waker_panicking_out_of_a_transitions_event_leaves_the_breaker_moved() {
        use std::task::{Context, Poll, Waker};

        use BreakerPhase::{Closed, HalfOpen, Open};

        let teller = Arc::new(Teller::new("upstream"));
        let breaker = Breaker::new(
            BreakerSettings {
                window: 1,
                failures: 1,
                open_for: OPEN_FOR,
                age: None,
                probes: 1,
            },
            Arc::clone(&teller),
        );
        let mut events = teller.subscribe();
        let waker = Waker::from(Arc::new(PanickingWaker));
        let mut reader = Context::from_waker(&waker);
        let transition = |from, to| {
            Poll::Ready(Some(ResilienceEvent::Transition {
                from,
                to,
                manual: false,
            }))
        };

        let mut next = Box::pin(events.next());
        assert!(next.as_mut().poll(&mut reader).is_pending());
        assert!(unwinds(|| attempt(&breaker, true)));
        assert_eq!(breaker.state(), "open");
        assert_eq!(next.as_mut().poll(&mut reader), transition(Closed, Open));
        drop(next);

        tokio::time::advance(OPEN_FOR).await;
        let mut next = Box::pin(events.next());
        assert!(next.as_mut().poll(&mut reader).is_pending());
        let probe = breaker.admit(BOUND).expect("the open time passed");
        assert!(unwinds(|| probe.send(BOUND, None).settle(None)));
        assert_eq!(breaker.state(), "open", "the probe's turn is free");
        assert_eq!(next.as_mut().poll(&mut reader), transition(Open, HalfOpen));
        drop(next);
        let mut next = Box::pin(events.next());
        assert_eq!(next.as_mut().poll(&mut reader), transition(HalfOpen, Open));
        drop(next);

        assert!(all_told(&breaker));
        a_later_call_closes(&breaker).await;
    }

    /// What a [`Cut`] encloses: its own extent, one inside another ending as
    /// the outer goes on, and one a panic unwinds out of ending there.
    #[test]
    fn a_cut_encloses_its_own_extent_and_ends_as_it_unwinds() {
        assert!(!Cut::encloses());
        Cut::dropping(|| {
            assert!(Cut::encloses());
            Cut::dropping(|| assert!(Cut::encloses()));
            assert!(Cut::encloses(), "the outer one goes on");
        });
        assert!(!Cut::encloses());
        assert!(unwinds(|| Cut::dropping(|| panic!("the drop panics"))));
        assert!(!Cut::encloses(), "ended as it unwound");
    }

    /// An attempt in flight dropped inside a [`Cut`] has timed out, whatever
    /// its own bound: a failure in the window, and a probe failing — where
    /// its caller's drop counts for nothing and hands a probe's turn on. One
    /// settled, ignored or admitted and not sent counts as it did.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_dropped_inside_a_cut_has_timed_out() {
        let lines = nest_rs_testing::LogCapture::install();
        let breaker = breaker(2, 2);
        let admit = || breaker.admit(BOUND).expect("a closed breaker admits");

        drop(admit().send(BOUND, None));
        assert_eq!(breaker.stats().outcomes, 0, "its caller's drop");
        Cut::dropping(|| drop(admit()));
        assert_eq!(breaker.stats().outcomes, 0, "never sent");
        let (settled, ignored) = (admit().send(BOUND, None), admit().send(BOUND, None));
        Cut::dropping(|| {
            settled.settle(None);
            ignored.ignore();
        });
        assert_eq!(
            (breaker.stats().outcomes, breaker.stats().failures),
            (1, 0),
            "each counted as it answered"
        );

        let far = Instant::now() + Duration::from_secs(3600);
        let (unbounded, bounded) = (admit().send(BOUND, None), admit().send(BOUND, Some(far)));
        Cut::dropping(|| drop(unbounded));
        assert_eq!(breaker.stats().failures, 1);
        Cut::dropping(|| drop(bounded));
        assert_eq!(breaker.state(), "open", "two timeouts opened it");
        let opened = lines.find(TARGET, MOVED).pop().expect("the opening");
        assert_eq!(opened.field("error").as_deref(), Some(TIMED_OUT));

        tokio::time::advance(OPEN_FOR).await;
        let pass = breaker.admit(BOUND).expect("the open time passed");
        Cut::dropping(|| drop(pass));
        let probe = breaker
            .admit(BOUND)
            .expect("a probe never sent hands its turn on")
            .send(BOUND, None);
        Cut::dropping(|| drop(probe));
        assert_eq!(
            (breaker.state(), refused_for(&breaker)),
            ("open", OPEN_FOR),
            "the probe failed, and the breaker is open for its whole open time"
        );
    }

    /// One more success of a full window of successes, counted with no age,
    /// changes nothing, and takes no lock; once a failure is in the window,
    /// each success is counted, the failure leaving it by the window's length
    /// as before, and the failures among the latest still open the breaker.
    /// With an age, each success is counted, its own instant aging from its
    /// answer.
    #[tokio::test(start_paused = true)]
    async fn a_quiet_window_counts_its_next_failure_and_every_success_after_it() {
        let stats = |outcomes, failures| BreakerStats { outcomes, failures };
        let breaker = breaker(4, 3);
        for counted in 1..=4 {
            attempt(&breaker, false);
            assert_eq!(breaker.stats(), stats(counted, 0));
        }
        attempt(&breaker, false);
        assert_eq!(breaker.stats(), stats(4, 0), "quiet");
        attempt(&breaker, true);
        assert_eq!(breaker.stats(), stats(4, 1));
        for _ in 0..3 {
            attempt(&breaker, false);
            assert_eq!(
                breaker.stats(),
                stats(4, 1),
                "the failure still in the window"
            );
        }
        attempt(&breaker, false);
        assert_eq!(
            breaker.stats(),
            stats(4, 0),
            "the failure left by the window's length"
        );
        attempt(&breaker, true);
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "closed");
        attempt(&breaker, true);
        assert_eq!(breaker.state(), "open", "three of the latest four failed");

        let age = Duration::from_secs(60);
        let aged = aged(4, 3, age);
        for _ in 0..4 {
            attempt(&aged, false);
        }
        tokio::time::advance(age / 2).await;
        attempt(&aged, false);
        attempt(&aged, false);
        tokio::time::advance(age / 2 + Duration::from_secs(1)).await;
        assert_eq!(
            aged.stats(),
            stats(2, 0),
            "the two successes counted at half the age outlive the first four"
        );
    }

    /// What `call` answers on a thread of its own while this thread holds
    /// `breaker`'s lock, or `None` for a call waiting for it: given up past
    /// a wait no call of these tests needs, so it fails a test rather than
    /// hang it.
    fn beside_the_lock<T: Send>(breaker: &Breaker, call: impl FnOnce() -> T + Send) -> Option<T> {
        let (answering, answer) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let held = breaker.published.lock();
            scope.spawn(move || {
                answering.send(call()).expect("the test reads the answer");
            });
            let answered = answer.recv_timeout(HANG).ok();
            drop(held);
            answered
        })
    }

    /// A closed breaker lets a call through, a quiet one counts one more
    /// success, and an open one refuses a call with what is left of its open
    /// time, each off the word the breaker published, while another thread
    /// holds the breaker's lock.
    #[test]
    fn a_closed_breaker_and_an_open_one_answer_while_another_thread_holds_its_lock() {
        let breaker = breaker(4, 2);
        assert_eq!(
            beside_the_lock(&breaker, || breaker.admit(BOUND).is_ok()),
            Some(true),
            "closed, a call is let through"
        );
        for _ in 0..4 {
            attempt(&breaker, false);
        }
        assert_eq!(
            beside_the_lock(&breaker, || attempt(&breaker, false)),
            Some(()),
            "quiet, one more success is counted"
        );
        assert_eq!(
            breaker.stats(),
            BreakerStats {
                outcomes: 4,
                failures: 0
            }
        );
        breaker.open_manually(OPEN_FOR);
        let refused = beside_the_lock(&breaker, || {
            (breaker.admit(BOUND).err(), breaker.refusing())
        });
        assert!(
            matches!(refused, Some((Some(left), Some(also)))
                if !left.is_zero() && left <= OPEN_FOR && !also.is_zero() && also <= OPEN_FOR),
            "open, a call is refused for what is left of the open time: {refused:?}"
        );
    }

    /// Ten threads calling through one breaker, near half their calls
    /// failing, beside a thread switching it by hand: whatever each reads
    /// with no lock and counts under it, the window never holds more outcomes
    /// than its length nor as many failures as open it, a refusal tells a
    /// wait, and an opening by hand refuses the next call, its window
    /// forgotten. The threads gone, as many failures as open the breaker open
    /// it, and one fewer does not.
    #[test]
    fn threads_failing_beside_a_switch_leave_the_window_within_its_bounds_and_exact() {
        const WINDOW: usize = 20;
        const FAILURES: usize = 10;
        let breaker = Breaker::new(
            BreakerSettings {
                window: WINDOW,
                failures: FAILURES,
                open_for: Duration::from_micros(10),
                age: None,
                probes: 1,
            },
            Arc::new(Teller::new("upstream")),
        );
        let (stop, violations) = (AtomicBool::new(false), AtomicUsize::new(0));
        // Every thread stops at the bound: past it, one telling transitions would tell
        // the others' forever.
        let started = std::time::Instant::now();
        let violated = |broken: bool| {
            if broken {
                violations.fetch_add(1, Ordering::Relaxed);
            }
        };
        let switched = std::thread::scope(|scope| {
            for thread in 1..=10_u64 {
                let (breaker, stop, violated) = (&breaker, &stop, &violated);
                scope.spawn(move || {
                    let mut drawn = thread.wrapping_mul(0x9E37_79B9_7F4A_7C15);
                    while !stop.load(Ordering::Relaxed) && started.elapsed() < HANG {
                        drawn ^= drawn << 13;
                        drawn ^= drawn >> 7;
                        drawn ^= drawn << 17;
                        let failed = drawn % 100 < 45;
                        match breaker.admit(BOUND) {
                            Ok(pass) => pass.send(BOUND, None).settle(failed.then_some(DOWN)),
                            Err(retry_after) => violated(retry_after.is_zero()),
                        }
                        let stats = breaker.stats();
                        violated(
                            stats.failures > stats.outcomes
                                || stats.outcomes > WINDOW
                                || stats.failures >= FAILURES,
                        );
                    }
                });
            }
            let mut switched = 0;
            while switched < SWITCHES && started.elapsed() < HANG {
                switched += 1;
                breaker.open_manually(Duration::from_secs(60));
                violated(breaker.admit(BOUND).is_ok());
                violated(!matches!(breaker.read(), BreakerState::Open { .. }));
                violated(breaker.stats().outcomes != 0);
                breaker.close_manually();
                for _ in 0..5 * WINDOW {
                    if let Ok(pass) = breaker.admit(BOUND) {
                        pass.send(BOUND, None).settle(None);
                    }
                }
            }
            stop.store(true, Ordering::Relaxed);
            switched
        });
        assert_eq!(
            switched, SWITCHES,
            "the switching thread starved of the lock"
        );
        assert_eq!(violations.into_inner(), 0);

        for _ in 0..SWITCHES / 10 {
            breaker.close_manually();
            for _ in 0..WINDOW {
                attempt(&breaker, false);
            }
            for _ in 1..FAILURES {
                attempt(&breaker, true);
            }
            assert_eq!(breaker.state(), "closed", "one failure short of opening");
            for _ in 1..WINDOW {
                attempt(&breaker, false);
            }
            assert_eq!(
                breaker.stats().failures,
                1,
                "the last failure still in the window"
            );
            attempt(&breaker, false);
            assert_eq!(breaker.stats().failures, 0);
            for _ in 0..FAILURES {
                attempt(&breaker, true);
            }
            assert_eq!(breaker.state(), "open");
        }
    }

    /// How many times the thread switching a breaker under load opens and
    /// closes it.
    const SWITCHES: usize = 6_000;

    /// A wall-clock bound no test here comes near but one waiting for a lock
    /// it should not take: past it, the test fails rather than hang.
    const HANG: Duration = Duration::from_secs(10);
}
