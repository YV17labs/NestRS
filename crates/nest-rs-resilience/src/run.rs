//! [`Run`] — one run of a call under its policy, whatever the call's type:
//! opened in the bulkhead, each attempt let through by the unit's deadline,
//! the breaker and the rate, sent under its bound, judged and counted in the
//! breaker, settled in its savepoint and sent again after its backoff — and
//! [`Running`], the one future a call's type is compiled into, which polls
//! that call through its run. The run polls the call behind [`Calling`], so it
//! is compiled once, whatever the calls an app makes.

use std::borrow::Cow;
use std::future::Future;
use std::num::NonZeroU32;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll, Waker, ready};
use std::time::Duration;

use nest_rs_core::__private::WithinHeld;
use nest_rs_core::{Backoff, Unanswered, process_seed};
use nest_rs_database::AtomicError;
use tokio::time::{Instant, Sleep};
use tokio_util::sync::DropGuard;

use crate::attempting::{Attempting, Lent, Owed, SharedSlot, SharedTurn, Taken};
use crate::breaker::{Breaker, Cut, Failure, Pass, Sent};
use crate::bulkhead::{Full, Slot};
use crate::call::{Calling, Erased, Judged, Placing, Reads};
use crate::error::{FAILED, PANICKED, ResilienceError, ResilienceRefusal, TIMED_OUT};
use crate::mechanisms::Mechanisms;
use crate::rate::{Arrived, Rate, Reservation};
use crate::resilient::{ResilientAnswer, ResilientMethod};
use crate::savepoint::{Savepoint, Settling, Then};
use crate::verdict::{Ended, Verdict};

/// What every attempt of one run is sent under, copied into the wait for a
/// token, so that no wait borrows its run.
#[derive(Clone, Copy)]
pub(crate) struct Terms<'p> {
    /// The policy's mechanisms, as the policy holds them, so what a run's
    /// answer holds past it is taken from this policy alone.
    pub(crate) policy: &'p Arc<Mechanisms>,
    /// The decorated method the run came from, which its lines name.
    pub(crate) method: Option<&'static ResilientMethod>,
    /// The deadline of the unit the run is in, which no attempt and no wait
    /// outlives: read as the run opens.
    deadline: Option<Instant>,
    /// How long each attempt waits: the policy's timeout, or the one its
    /// method pinned.
    timeout: Duration,
}

/// How long an attempt sent now may run, and, under a unit's deadline, the
/// instant that ends at: its timer is armed at that instant, so it is due with
/// the edge's cut at the deadline however long the call takes to first wait,
/// and an attempt the edge cuts there first has timed out all the same
/// ([`Sent`]). Under no deadline it ends at no instant, and an attempt its
/// caller drops past it counts for nothing. Under either, an attempt dropped
/// by the timeout of a policy around it has timed out, whatever its own bound
/// has left ([`Cut`]).
#[derive(Clone, Copy)]
struct Bound {
    wait: Duration,
    until: Option<Instant>,
}

/// What an attempt meets as it is let through.
enum Admission<'p> {
    /// Let through, its call sent under the bound, holding the breaker's pass.
    Through(Option<Pass<'p>>, Bound),
    Refused(ResilienceRefusal),
    /// No token of the rate free, postponed to the instant its reserved one is.
    Postponed(Instant),
}

impl<'p> Terms<'p> {
    /// How long an attempt sent now may run: the run's timeout, or what is left
    /// of the unit's deadline when that is shorter.
    fn bound(&self) -> Bound {
        if self.deadline.is_none() {
            return Bound {
                wait: self.timeout,
                until: None,
            };
        }
        let now = Instant::now();
        let wait = self.bound_at(now);
        Bound {
            wait,
            until: Some(now + wait),
        }
    }

    /// How long an attempt sent at `at` may run.
    fn bound_at(&self, at: Instant) -> Duration {
        self.deadline.map_or(self.timeout, |deadline| {
            self.timeout.min(deadline.saturating_duration_since(at))
        })
    }

    /// `refusal`, which the run ends on, counted and told.
    fn refused(&self, refusal: ResilienceRefusal) -> ResilienceRefusal {
        self.policy.teller.refused(self.method, refusal)
    }

    /// An attempt let through, in order, by what is left of the unit's
    /// deadline, the breaker and the rate, with the bound its call is sent
    /// under; with nothing left of the deadline it is not sent, and takes no
    /// token. Finding no token of the rate free it reserves the next and
    /// waits for it, when that one comes within the rate's wait and before
    /// the unit's deadline — a wait that is no part of the attempt —
    /// `reserved` holding its place in the line and a probe's `pass` keeping
    /// its turn, telling the breaker when it has answered at the latest;
    /// otherwise it is refused, taking nothing, told the time to that token.
    /// With no rate, nothing of the rate's runs: no lock, no clock read, no
    /// timer. A call sharing the turn of the attempt it is made in, `shares`
    /// naming the epoch that attempt was let through in, asks the breaker
    /// nothing while the breaker's published word is still in that epoch, and
    /// meets the breaker as any call does once it opened or was switched by
    /// hand since; one holding that attempt's `token` takes none.
    // Inlined, the rate's step alone left out of line: a run with no rate pays no
    // call to be let through.
    #[inline(always)]
    fn admit(
        &self,
        pass: &mut Option<Pass<'p>>,
        reserved: &mut Option<Reservation<'p>>,
        shares: Option<u64>,
        token: bool,
    ) -> Admission<'p> {
        let bound = self.bound();
        if bound.wait.is_zero() {
            return Admission::Refused(ResilienceRefusal::OUT_OF_TIME);
        }
        let breaker = self.policy.breaker.as_ref();
        let shared =
            shares.is_some_and(|epoch| breaker.is_some_and(|breaker| breaker.current(epoch)));
        // While this one waited for its token, another call may have opened the
        // breaker or taken the probe's turn, and a manual switch voids a turn held.
        if !shared && !pass.as_ref().is_some_and(Pass::probes) {
            match breaker.map(|breaker| breaker.admit(bound.wait)).transpose() {
                Ok(admitted) => *pass = admitted,
                Err(retry_after) => {
                    return Admission::Refused(ResilienceRefusal::Open { retry_after });
                }
            }
        }
        match &self.policy.rate {
            Some(rate) if !token => self.token(rate, pass, reserved, bound),
            _ => Admission::Through(pass.take(), bound),
        }
    }

    /// [`admit`](Self::admit)'s step through `rate`, for an attempt the
    /// breaker let through to be sent under `bound`: the token it `reserved`
    /// taken, or one asked for as it arrives.
    #[inline(never)]
    fn token(
        &self,
        rate: &'p Rate,
        pass: &mut Option<Pass<'p>>,
        reserved: &mut Option<Reservation<'p>>,
        bound: Bound,
    ) -> Admission<'p> {
        let now = Instant::now();
        if let Some(reservation) = reserved.take() {
            return match reservation.take(now) {
                Ok(()) => Admission::Through(pass.take(), bound),
                Err((reservation, at)) => {
                    *reserved = Some(reservation);
                    Admission::Postponed(at)
                }
            };
        }
        match rate.arrive(now, self.deadline) {
            Arrived::Taken => Admission::Through(pass.take(), bound),
            Arrived::Refused(wait) => {
                self.policy.teller.limited(self.method, wait);
                Admission::Refused(ResilienceRefusal::Limited { retry_after: wait })
            }
            Arrived::Reserved(reservation, at) => {
                *reserved = Some(reservation);
                if let Some(pass) = pass.as_ref() {
                    pass.postpone(at, self.bound_at(at));
                }
                let wait = at.saturating_duration_since(now);
                self.policy.teller.waiting_for_token(self.method, wait);
                Admission::Postponed(at)
            }
        }
    }
}

pin_project_lite::pin_project! {
    /// An attempt that found no token of the rate free, waiting for the one
    /// it reserved, then let through again as [`Terms::admit`] says. Dropped
    /// before then — its run dropped, its breaker refusing it or its unit out
    /// of time once the wait ends — it gives the token to the attempt behind.
    struct Admitted<'p> {
        terms: Terms<'p>,
        // `reserved` before `pass`: given back first, so the call a probe's turn
        // is handed to finds the token this one left.
        reserved: Option<Reservation<'p>>,
        pass: Option<Pass<'p>>,
        // The epoch the turn the call shares was let through in, when it shares
        // the turn of the attempt it is made in.
        shares: Option<u64>,
        #[pin]
        due: Sleep,
    }
}

impl<'p> Future for Admitted<'p> {
    type Output = Result<(Option<Pass<'p>>, Bound), ResilienceRefusal>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        loop {
            // A reservation given back before this one brings its token forward,
            // and wakes it: its timer and a probe's latest answer move with it.
            if let Some(reservation) = this.reserved.as_ref() {
                let at = reservation.due(cx.waker());
                if at < this.due.deadline() {
                    if let Some(pass) = this.pass.as_ref() {
                        pass.postpone(at, this.terms.bound_at(at));
                    }
                    this.due.as_mut().reset(at);
                }
            }
            ready!(this.due.as_mut().poll(cx));
            match this
                .terms
                .admit(this.pass, this.reserved, *this.shares, false)
            {
                Admission::Through(pass, bound) => return Poll::Ready(Ok((pass, bound))),
                Admission::Refused(refusal) => return Poll::Ready(Err(refusal)),
                Admission::Postponed(at) => this.due.as_mut().reset(at),
            }
        }
    }
}

/// A call's wait for a slot of the bulkhead it found none free of.
type Entering<'p> = Pin<Box<dyn Future<Output = Result<Slot<'p>, Full>> + Send + 'p>>;

pin_project_lite::pin_project! {
    /// What a run waits on besides its call, one at a time: the bound of its
    /// attempt, whose timer is armed once the call waits, and only when they
    /// come, the slot of the bulkhead it found none free of, the rate's token
    /// and the backoff before a retry. The two waits a call may never meet
    /// are boxed, so a run reserves no room for them.
    #[project = WaitProj]
    enum Wait<'p> {
        Within {
            #[pin]
            within: WithinHeld,
        },
        Entering {
            entering: Entering<'p>,
        },
        Admitting {
            admitted: Pin<Box<Admitted<'p>>>,
        },
        BackingOff {
            #[pin]
            backoff: Sleep,
        },
    }
}

impl Wait<'_> {
    const fn within() -> Self {
        Self::Within {
            within: WithinHeld::new(Duration::ZERO, None),
        }
    }
}

/// Where a run is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Not opened yet.
    Opening,
    /// Waiting for a slot of the bulkhead.
    Entering,
    /// Its next attempt not let through yet.
    Arriving,
    /// Waiting for the token of the rate its attempt reserved.
    Admitting,
    /// Its attempt's call sent.
    Sending,
    /// Waiting for its backoff before a retry.
    BackingOff,
    /// Over.
    Answered,
}

/// A retry decided after an attempt: how long the run waits before it,
/// counted from that attempt's answer, and what its line and its event say
/// the attempt answered in error.
pub(crate) struct Retry {
    wait: Duration,
    error: Told,
}

/// What a retry's line and event say the attempt before it answered.
enum Told {
    /// An `Ok` sent again: no error.
    Answered,
    /// An error no line and no subscription heard as the retry was decided,
    /// so never put in words: the retry tells nothing.
    Unheard,
    /// An error's chain, or the timeout's sentence.
    Said(Cow<'static, str>),
}

/// What a run ended on, its last attempt's answer, when it has one, held by
/// its call.
pub(crate) enum Over {
    /// The call's own answer, its last attempt's.
    Answered,
    /// The policy's refusal, counted and told.
    Refused(ResilienceRefusal),
    /// The savepoint of its last attempt, which the database could not
    /// settle.
    Unsettled(AtomicError),
}

/// A run's last attempt, as its breaker holds it.
enum Attempt<'p> {
    /// Its call sent, its outcome to come: dropped so, it is judged by what
    /// dropped it ([`Sent`]).
    Sent(Sent<'p>),
    /// Its answer judged a success, which what holds that answer past the run
    /// — a response's body — may still undo: dropped so, the success stands.
    #[cfg(feature = "reqwest")]
    Outstanding(Outstanding<'p>),
    /// Sent on the turn of the attempt it is made in, a call of that
    /// attempt's policy: its failure is that attempt's once the run ends on
    /// it, and one the run sends again past is owed until then.
    Shared(SharedTurn),
}

/// A success left in the breaker, settled as it was judged once dropped.
#[cfg(feature = "reqwest")]
struct Outstanding<'p>(Option<Sent<'p>>);

#[cfg(feature = "reqwest")]
impl Drop for Outstanding<'_> {
    fn drop(&mut self) {
        if let Some(sent) = self.0.take() {
            sent.settle(None);
        }
    }
}

/// One poll of an attempt's call, lending the calls of its policy it makes
/// what the attempt shares — its slot, its turn and its token — and taking it
/// back, as those calls left it, as the poll ends or unwinds.
struct Lending<'r> {
    attempting: Attempting,
    lent: &'r mut Lent,
}

impl<'r> Lending<'r> {
    #[inline]
    fn enter(policy: &Mechanisms, again: bool, lent: &'r mut Lent) -> Self {
        Self {
            attempting: Attempting::enter(policy, again, lent),
            lent,
        }
    }
}

impl Drop for Lending<'_> {
    #[inline]
    fn drop(&mut self) {
        if let Some(back) = self.attempting.leave() {
            *self.lent = back;
        }
    }
}

pin_project_lite::pin_project! {
    /// One run of a call under its policy.
    pub(crate) struct Run<'p> {
        // Dropped in order: the attempt in the breaker, handing on a probe's turn,
        // and what an attempt sent again failed on the turn it shared, then its
        // token, its savepoint, what the run waits on — a reservation, then a
        // probe's pass — and its slot of the bulkhead, its own or the one it
        // shares.
        attempt: Option<Attempt<'p>>,
        // What an attempt sent again failed on the turn it shared, the failure
        // of the attempt the run is made in only should the run end failing.
        owed: Option<Box<Owed>>,
        given_up: Option<DropGuard>,
        savepoint: Option<Savepoint>,
        settling: Option<Box<Settling>>,
        #[pin]
        wait: Wait<'p>,
        slot: Option<Slot<'p>>,
        // The slot of the attempt the run is made in, shared while the run holds
        // none of its own.
        shared_slot: Option<SharedSlot>,
        terms: Terms<'p>,
        // The bound the last attempt was sent under.
        bound: Duration,
        // How many times the run may send its call again: the policy's retries,
        // or the ones its method pinned.
        retries: u32,
        // Drawn at the first retry, so a call answering at once draws none.
        seed: Option<u64>,
        // How many times the run attempted its call again so far, a retry the
        // policy refused unsent included.
        resent: u32,
        stage: Stage,
        // Whether the call is declared idempotent: it is sent again, as its
        // judging says.
        idempotent: bool,
        // Whether each attempt runs in a savepoint of its unit's transaction.
        savepoints: bool,
        // Whether an answer judged a success is left in the breaker for what
        // holds the answer past the run to settle — a response's body.
        holds: bool,
        // When the last attempt was sent, for a run that holds its answer.
        sent_at: Option<Instant>,
        // Whether the run opened inside an attempt of its own policy, a call that
        // attempt makes.
        nested: bool,
        // Whether that attempt may still be sent again, by its run or one around
        // it: the run is then sent once, the attempt around it being what is
        // sent again.
        again_around: bool,
        // What the last attempt shares with the calls of its policy it makes.
        lent: Lent,
    }
}

impl<'p> Run<'p> {
    /// One run of `method`'s call, sent once or, `idempotent`, again; each of
    /// its attempts in a savepoint of its unit's transaction when the call is
    /// declared `idempotent` or its method declares a fallback: read off the
    /// declaration alone, so the retries a deployment sets never change what
    /// a failed call leaves in its unit.
    pub(crate) fn new(
        policy: &'p Arc<Mechanisms>,
        method: Option<&'static ResilientMethod>,
        idempotent: bool,
    ) -> Self {
        Self {
            attempt: None,
            owed: None,
            given_up: None,
            savepoint: None,
            settling: None,
            wait: Wait::within(),
            slot: None,
            shared_slot: None,
            terms: Terms {
                policy,
                method,
                deadline: None,
                timeout: method
                    .and_then(|method| method.timeout())
                    .unwrap_or(policy.timeout),
            },
            bound: Duration::ZERO,
            retries: method
                .and_then(|method| method.retries())
                .unwrap_or(policy.retries),
            seed: None,
            resent: 0,
            stage: Stage::Opening,
            idempotent,
            savepoints: idempotent || method.is_some_and(|method| method.fallback()),
            holds: false,
            sent_at: None,
            nested: false,
            again_around: false,
            lent: Lent::default(),
        }
    }

    /// The run of a transport's call — a request the middleware sends —
    /// which opens no savepoint, is sent again as its transport decides, and
    /// leaves an answer judged a success in the breaker for what holds the
    /// answer past the run to settle.
    #[cfg(feature = "reqwest")]
    pub(crate) fn holding(policy: &'p Arc<Mechanisms>) -> Self {
        Self {
            savepoints: false,
            holds: true,
            ..Self::new(policy, None, true)
        }
    }

    /// The instance its policy is named for, which a refusal it answers names.
    pub(crate) fn instance(&self) -> &'static str {
        self.terms.policy.teller.instance()
    }

    /// How many times the run attempted its call again so far.
    #[cfg(feature = "reqwest")]
    pub(crate) const fn resent(&self) -> u32 {
        self.resent
    }

    /// Whether the run may still send its call again.
    #[cfg(feature = "reqwest")]
    pub(crate) const fn may_resend(&self) -> bool {
        self.resent < self.retries
    }

    /// The run opened under the unit's deadline, holding a free slot of the
    /// bulkhead, refused at once, in an attempt's order, by a unit out of time
    /// and then by an open breaker — or, finding no slot free, once one is
    /// freed within that deadline. The rate is read first then, taking
    /// nothing: a call no wait it may make brings to its token — the one
    /// behind the attempts waiting, past the longest it waits for a slot and
    /// the rate's wait from there, or at the unit's deadline — is refused at
    /// once rather than after its wait for a slot. Any other waits, and meets
    /// the rate as it stands once it holds its slot: refused there by a token
    /// still past the rate's wait. It is refused `Full` once no slot is freed
    /// for it in time or as many calls wait for one as may. A refusal is
    /// counted and told. Opened inside an attempt of its own policy, it shares
    /// that attempt's slot while no other call does; otherwise it is opened
    /// so, and takes the attempt's slot should it come back before one is
    /// freed within the bulkhead's wait — with none, it is refused `Full` at
    /// once. Opened inside an attempt its policy may still send again, the
    /// run sends its call once, decided here: should that attempt then end
    /// without being sent again — its predicate refusing, its retry given up,
    /// the call's error swallowed — the call was still sent once; inside
    /// one never sent again — a run once, or the last attempt of a run — it
    /// keeps its own retries, which nothing around it multiplies, and fails
    /// that attempt only should it end failing.
    pub(crate) fn poll_open(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), ResilienceRefusal>> {
        let mut this = self.project();
        if *this.stage == Stage::Opening {
            let policy = this.terms.policy;
            policy.observed.at_run();
            this.terms.deadline = nest_rs_core::__private::deadline();
            *this.stage = Stage::Arriving;
            if let Some(again) = Attempting::inside(policy) {
                *this.nested = true;
                if again {
                    *this.again_around = true;
                    *this.retries = 0;
                }
                // The attempt's own slot, which a bulkhead of one would otherwise refuse it.
                if policy.bulkhead.is_some()
                    && let Some(shared) = Attempting::slot(policy)
                {
                    *this.shared_slot = Some(shared);
                    return Poll::Ready(Ok(()));
                }
            }
            let Some(bulkhead) = &policy.bulkhead else {
                return Poll::Ready(Ok(()));
            };
            let deadline = this.terms.deadline;
            if deadline.is_some_and(|deadline| deadline <= Instant::now()) {
                *this.stage = Stage::Answered;
                return Poll::Ready(Err(this.terms.refused(ResilienceRefusal::OUT_OF_TIME)));
            }
            if let Some(retry_after) = policy.breaker.as_ref().and_then(Breaker::refusing) {
                *this.stage = Stage::Answered;
                return Poll::Ready(Err(this
                    .terms
                    .refused(ResilienceRefusal::Open { retry_after })));
            }
            if let Some(free) = bulkhead.try_enter() {
                *this.slot = Some(free);
                return Poll::Ready(Ok(()));
            }
            if let Some(rate) = &policy.rate
                && let Some(wait) = rate.refusing(Instant::now(), bulkhead.longest_wait(), deadline)
            {
                *this.stage = Stage::Answered;
                policy.teller.limited(this.terms.method, wait);
                return Poll::Ready(Err(this
                    .terms
                    .refused(ResilienceRefusal::Limited { retry_after: wait })));
            }
            this.wait.set(Wait::Entering {
                entering: Box::pin(bulkhead.enter(deadline)),
            });
            *this.stage = Stage::Entering;
        }
        if *this.stage == Stage::Entering {
            if *this.nested {
                let policy = this.terms.policy;
                if let Some(shared) = Attempting::slot(policy) {
                    this.wait.set(Wait::within());
                    *this.shared_slot = Some(shared);
                    *this.stage = Stage::Arriving;
                    return Poll::Ready(Ok(()));
                }
                Attempting::wait_for_slot(policy, cx.waker());
            }
            let WaitProj::Entering { entering } = this.wait.as_mut().project() else {
                unreachable!("a run waits for the slot it found none free of");
            };
            let entered = ready!(entering.as_mut().poll(cx));
            this.wait.set(Wait::within());
            match entered {
                Ok(slot) => {
                    *this.slot = Some(slot);
                    *this.stage = Stage::Arriving;
                }
                Err(full) => {
                    *this.stage = Stage::Answered;
                    let terms = this.terms;
                    terms.policy.teller.full(terms.method, full.waiters());
                    return Poll::Ready(Err(terms.refused(ResilienceRefusal::Full)));
                }
            }
        }
        Poll::Ready(Ok(()))
    }

    /// One attempt of the run's call: let through as [`Terms::admit`] says,
    /// waiting for the rate's token only when it must, its call made and sent
    /// under its bound, counted in the breaker once it answers and its
    /// cancellation token, when its call takes one, cancelled when it is given
    /// up. A panic in the call, or in a predicate judging its answer, counts
    /// as a failure, and is handed back for the run to roll the attempt's
    /// savepoint back before it unwinds on. A run opened inside an attempt of
    /// its own policy shares that attempt's turn of the breaker, a probe's
    /// included, while no other call does, and takes its token of the rate
    /// when no call took it: it then asks the rate nothing, and the breaker
    /// nothing while it is in the epoch that attempt was let through in. Its
    /// failure there fails that attempt once the run ends on it: one it sends
    /// again past is owed, and forgiven should a later attempt answer.
    pub(crate) fn poll_attempt(
        mut self: Pin<&mut Self>,
        mut call: Pin<&mut (dyn Calling + '_)>,
        cx: &mut Context<'_>,
    ) -> Poll<Ended> {
        if self.stage == Stage::Arriving {
            let Taken { turn, token } = if self.nested {
                Attempting::take(self.terms.policy)
            } else {
                Taken::NOTHING
            };
            let shares = turn.as_ref().map(SharedTurn::epoch);
            let (mut pass, mut reserved) = (None, None);
            let admitted = self.terms.admit(&mut pass, &mut reserved, shares, token);
            if let Some(turn) = turn {
                *self.as_mut().project().attempt = Some(Attempt::Shared(turn));
            }
            match admitted {
                Admission::Through(pass, bound) => self.as_mut().send(pass, bound),
                Admission::Refused(refusal) => {
                    // A turn shared and not sent on goes back at once.
                    *self.as_mut().project().attempt = None;
                    return Poll::Ready(Ended::Refused(refusal));
                }
                Admission::Postponed(at) => {
                    let mut this = self.as_mut().project();
                    let admitted = Box::pin(Admitted {
                        terms: *this.terms,
                        reserved,
                        pass,
                        shares,
                        due: tokio::time::sleep_until(at),
                    });
                    this.wait.set(Wait::Admitting { admitted });
                    *this.stage = Stage::Admitting;
                }
            }
        }
        if self.stage == Stage::Admitting {
            let mut this = self.as_mut().project();
            let WaitProj::Admitting { admitted } = this.wait.as_mut().project() else {
                unreachable!("an attempt waits for the token it reserved");
            };
            let admitted = ready!(admitted.as_mut().poll(cx));
            this.wait.set(Wait::within());
            *this.stage = Stage::Arriving;
            match admitted {
                Ok((pass, bound)) => self.as_mut().send(pass, bound),
                Err(refusal) => {
                    *self.as_mut().project().attempt = None;
                    return Poll::Ready(Ended::Refused(refusal));
                }
            }
        }
        let this = self.as_mut().project();
        let WaitProj::Within { within } = this.wait.project() else {
            unreachable!("an attempt is sent and answers while its run waits on nothing else");
        };
        let mut erased = Erased {
            call: call.as_mut(),
            given_up: this.given_up,
        };
        let again = *this.again_around || (*this.idempotent && *this.resent < *this.retries);
        let polled = {
            let _lending = Lending::enter(this.terms.policy, again, this.lent);
            match this.savepoint {
                Some(savepoint) => within.poll(pin!(savepoint.around(Pin::new(&mut erased))), cx),
                None => within.poll(Pin::new(&mut erased), cx),
            }
        };
        let answered = ready!(polled);
        Poll::Ready(self.settle(call, answered))
    }

    /// The attempt let through with `pass`, its outcome in the breaker, or on
    /// the turn it shares, under `bound`: its call is made at its first poll,
    /// inside the bound, so one panicking before it hands back its future
    /// fails the attempt as a panic while it runs does. A retry counts on the
    /// retries' counter.
    fn send(self: Pin<&mut Self>, pass: Option<Pass<'p>>, bound: Bound) {
        let this = self.project();
        if *this.resent > 0 {
            this.terms.policy.teller.resent();
        }
        if let Some(pass) = pass {
            *this.attempt = Some(Attempt::Sent(pass.send(bound.wait, bound.until)));
        }
        let turn = match this.attempt {
            Some(Attempt::Sent(sent)) => Some(sent.epoch()),
            Some(Attempt::Shared(turn)) => Some(turn.epoch()),
            #[cfg(feature = "reqwest")]
            Some(Attempt::Outstanding(Outstanding(sent))) => sent.as_ref().map(Sent::epoch),
            None => None,
        };
        this.lent.renew(
            this.slot.is_some() || this.shared_slot.is_some(),
            turn,
            this.terms.policy.rate.is_some(),
        );
        *this.bound = bound.wait;
        if *this.holds {
            *this.sent_at = Some(Instant::now());
        }
        let WaitProj::Within { within } = this.wait.project() else {
            unreachable!("an attempt is sent while its run waits on nothing else");
        };
        within.reset(bound.wait, bound.until);
        if *this.savepoints {
            *this.savepoint = Savepoint::open();
        }
        *this.stage = Stage::Sending;
    }

    /// What the attempt ended on, `answered` judged and counted in the
    /// breaker, the future the call made dropped first, so before its
    /// savepoint settles. The answer is judged before the breaker is asked, so
    /// no `Display` of the call's error runs under its lock; a predicate that
    /// panics fails the attempt as a panic in its call does.
    fn settle(
        self: Pin<&mut Self>,
        mut call: Pin<&mut (dyn Calling + '_)>,
        answered: Result<(), Unanswered>,
    ) -> Ended {
        let this = self.project();
        *this.stage = Stage::Arriving;
        match &answered {
            Ok(()) | Err(Unanswered::Panicked(_)) => call.as_mut().spend(),
            // An attempt of another policy in flight in the call is dropped by
            // this timeout, never by its caller.
            Err(_elapsed) => Cut::dropping(|| call.as_mut().spend()),
        }
        let idempotent = *this.idempotent;
        let judged = answered.and_then(|()| {
            catch_unwind(AssertUnwindSafe(|| call.judge(idempotent))).map_err(Unanswered::Panicked)
        });
        let mut judged = judged;
        // A call that shared the attempt's turn failed: the attempt has failed,
        // whatever it answered.
        let shared = this.lent.failed();
        let failed = match &judged {
            Ok(Verdict::Failed { .. } | Verdict::Final) => {
                Some(call.failure().unwrap_or(Failure::Said(FAILED)))
            }
            Ok(_) => shared.as_deref().map(Failure::Told),
            Err(Unanswered::Panicked(_)) => Some(Failure::Said(PANICKED)),
            Err(_) => Some(Failure::Said(TIMED_OUT)),
        };
        if failed.is_none()
            && let Some(owed) = this.owed.take()
        {
            owed.forgive();
        }
        let counted = match this.attempt.take() {
            Some(Attempt::Sent(sent)) => match (failed, &judged) {
                (Some(failure), _) => sent.ended(Some(failure)),
                // What the answer still owes — a response's body — may undo it.
                #[cfg(feature = "reqwest")]
                (None, Ok(Verdict::Succeeded)) if *this.holds => {
                    *this.attempt = Some(Attempt::Outstanding(Outstanding(Some(sent))));
                    Ok(())
                }
                (None, Ok(Verdict::Ignored | Verdict::IgnoredAgain { .. })) => {
                    sent.ignore();
                    Ok(())
                }
                (None, _) => sent.ended(None),
            },
            Some(Attempt::Shared(turn)) => match failed.map(Failure::said).transpose() {
                // What the answer still owes — a response's body — may fail it.
                #[cfg(feature = "reqwest")]
                Ok(None) if *this.holds && matches!(judged, Ok(Verdict::Succeeded)) => {
                    *this.attempt = Some(Attempt::Shared(turn));
                    Ok(())
                }
                // Sent again past it, the run owes that failure to the attempt it
                // is made in only should it end failing.
                Ok(Some(said))
                    if idempotent
                        && *this.resent < *this.retries
                        && matches!(
                            judged,
                            Ok(Verdict::Failed { .. }) | Err(Unanswered::Elapsed(_))
                        ) =>
                {
                    if let Some(owed) = this.owed.replace(Box::new(turn.owing(said))) {
                        owed.forgive();
                    }
                    Ok(())
                }
                Ok(said) => {
                    if let Some(owed) = this.owed.take() {
                        owed.forgive();
                    }
                    turn.ended(said);
                    Ok(())
                }
                Err(payload) => {
                    if let Some(owed) = this.owed.take() {
                        owed.forgive();
                    }
                    turn.ended(Some(Cow::Borrowed(PANICKED)));
                    Err(payload)
                }
            },
            #[cfg(feature = "reqwest")]
            Some(Attempt::Outstanding(_)) => unreachable!("an attempt settles once"),
            None => Ok(()),
        };
        if let Err(payload) = counted {
            judged = Err(Unanswered::Panicked(payload));
        }
        let given_up = this.given_up.take();
        match judged {
            Ok(verdict) => {
                let _token = given_up.map(DropGuard::disarm);
                Ended::Judged(verdict)
            }
            Err(Unanswered::Panicked(payload)) => Ended::Unwound(payload),
            Err(_elapsed) => Ended::Refused(ResilienceRefusal::TimedOut {
                timeout: *this.bound,
            }),
        }
    }

    /// The retry the run makes after an attempt that `ended` so, or `None`
    /// when it ends there: after a timeout or an answer judged one to send
    /// again, unless its retries are spent or the retry is given up, whose
    /// line says why — the dependency asked a wait past the backoff's
    /// ceiling, or the wait ends past the unit's deadline — with what the
    /// attempt answered in error, its `failure`, which an `Ok` sent again has
    /// none of. That error is put in words only when a line or a subscription
    /// hears it; words that panic end the run there, `ended` then the panic
    /// it unwinds on. The retry's own line is left to file once it is
    /// certain ([`retrying`](Self::retrying)): at once for an attempt that
    /// opened no savepoint, else once that savepoint was rolled back.
    pub(crate) fn retry(
        self: Pin<&mut Self>,
        ended: &mut Ended,
        failure: Option<Failure<'_>>,
    ) -> Option<Retry> {
        let this = self.project();
        if !*this.idempotent {
            return None;
        }
        let (asked, failure) = sent_again(ended, failure)?;
        let terms = *this.terms;
        let policy = terms.policy;
        if *this.resent >= *this.retries {
            return None;
        }
        let error = match failure {
            None => Told::Answered,
            Some(_) if !policy.teller.hears() => Told::Unheard,
            Some(failure) => match failure.said() {
                Ok(said) => Told::Said(said),
                Err(payload) => {
                    *ended = Ended::Unwound(payload);
                    return None;
                }
            },
        };
        let said = match &error {
            Told::Said(said) => Some(&**said),
            Told::Answered | Told::Unheard => None,
        };
        let ceiling = policy.backoff.ceiling();
        if let Some(asked) = asked.filter(|asked| *asked > ceiling) {
            policy
                .teller
                .past_ceiling(terms.method, asked, ceiling, said);
            return None;
        }
        let seed = *this.seed.get_or_insert_with(|| {
            let run = policy.runs.fetch_add(1, Ordering::Relaxed);
            process_seed() ^ Backoff::seed(&run.to_le_bytes())
        });
        let failed = NonZeroU32::MIN.saturating_add(*this.resent);
        let backoff = policy.backoff.wait(failed, seed);
        let wait = asked.map_or(backoff, |asked| {
            backoff.max(Backoff::honouring(asked, failed, seed))
        });
        if let Some(deadline) = terms.deadline
            && Instant::now() + wait >= deadline
        {
            policy.teller.past_deadline(terms.method, wait, said);
            return None;
        }
        if let Ended::Refused(timed_out) = ended {
            // Given up at its bound, told as it ends: its run goes on.
            policy.teller.given_up(terms.method, *timed_out);
        }
        Some(Retry { wait, error })
    }

    /// The line and the event of a retry that is certain, saying what the
    /// attempt before it answered: `attempt failed`, with its error's chain or
    /// the timeout's sentence, for an `Err` or a timeout, whichever way the
    /// `Err` counts in the breaker, and `attempt answered`, with no error, for
    /// an `Ok` sent again — and nothing for an error nothing heard as the
    /// retry was decided.
    pub(crate) fn retrying(&self, retry: &Retry) {
        let terms = &self.terms;
        let error = match &retry.error {
            Told::Unheard => return,
            Told::Answered => None,
            Told::Said(said) => Some(&**said),
        };
        terms.policy.teller.retrying(
            terms.method,
            self.resent.saturating_add(1),
            retry.wait,
            error,
        );
    }

    /// The backoff before `retry` armed, counted from now: from the answer of
    /// the attempt it follows, the rollback of that attempt's savepoint
    /// inside it.
    pub(crate) fn back_off(self: Pin<&mut Self>, retry: &Retry) {
        let mut this = self.project();
        this.wait.set(Wait::BackingOff {
            backoff: tokio::time::sleep(retry.wait),
        });
        *this.stage = Stage::BackingOff;
    }

    /// The backoff waited out, and the next attempt counted as a retry.
    pub(crate) fn poll_back_off(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut this = self.project();
        if let WaitProj::BackingOff { backoff } = this.wait.as_mut().project() {
            ready!(backoff.poll(cx));
            this.wait.set(Wait::within());
            *this.resent = this.resent.saturating_add(1);
            *this.stage = Stage::Arriving;
        }
        Poll::Ready(())
    }

    /// `refusal`, which the run ends on, counted and told — as its last
    /// attempt ends, before its savepoint settles: an edge cutting the unit at
    /// the deadline the attempt was given up at drops the run while it rolls
    /// back, and the breaker has counted that timeout.
    pub(crate) fn finish(&self, refusal: ResilienceRefusal) -> ResilienceRefusal {
        self.terms.refused(refusal)
    }

    /// The whole run of `call`: opened, then each attempt and each wait
    /// between, every savepoint settled before the next attempt and before
    /// the run is over. A savepoint the database could not settle ends the
    /// run there, whatever the attempt answered — a timeout it was given up at
    /// counted and told already, as the attempt's: nothing that attempt wrote
    /// stands. Its line tells why, the cause's chain as its `error`: a driver
    /// owes no line of its own for it, and the run's caller alone is told,
    /// never past a panic unwinding on.
    fn poll_run(
        mut self: Pin<&mut Self>,
        mut call: Pin<&mut (dyn Calling + '_)>,
        cx: &mut Context<'_>,
    ) -> Poll<Over> {
        if matches!(self.stage, Stage::Opening | Stage::Entering)
            && let Err(refusal) = ready!(self.as_mut().poll_open(cx))
        {
            return Poll::Ready(Over::Refused(refusal));
        }
        loop {
            if let Some(settling) = self.as_mut().project().settling.as_mut() {
                let (then, settled) = ready!(settling.poll(cx));
                *self.as_mut().project().settling = None;
                if let Err(cause) = &settled {
                    let terms = &self.terms;
                    terms.policy.teller.unsettled(terms.method, cause);
                }
                match (then, settled) {
                    (Then::Unwound(payload), _) => resume_unwind(payload),
                    (_, Err(cause)) => return Poll::Ready(Over::Unsettled(cause)),
                    (Then::Answer, Ok(())) => return Poll::Ready(Over::Answered),
                    (Then::Refused(refusal), Ok(())) => return Poll::Ready(Over::Refused(refusal)),
                    (Then::Again(retry), Ok(())) => self.retrying(&retry),
                }
            }
            if self.stage == Stage::BackingOff {
                ready!(self.as_mut().poll_back_off(cx));
            }
            let mut ended = ready!(self.as_mut().poll_attempt(call.as_mut(), cx));
            let retry = match ended {
                Ended::Unwound(_) => None,
                _ => {
                    let failure = call.failure();
                    self.as_mut().retry(&mut ended, failure)
                }
            };
            let savepoint = self.as_mut().project().savepoint.take();
            if let Ended::Unwound(payload) = ended {
                // What a predicate that panicked left, let go before the rollback.
                call.as_mut().let_go();
                let Some(savepoint) = savepoint else {
                    resume_unwind(payload);
                };
                *self.as_mut().project().settling =
                    Some(savepoint.settle(false, Then::Unwound(payload)));
                continue;
            }
            let Some(retry) = retry else {
                let (then, keep) = match ended {
                    Ended::Judged(verdict) => (Then::Answer, verdict.answered_ok()),
                    Ended::Refused(refusal) => (Then::Refused(self.finish(refusal)), false),
                    Ended::Unwound(_) => unreachable!("a call that unwound is no answer"),
                };
                let Some(savepoint) = savepoint else {
                    return Poll::Ready(match then {
                        Then::Refused(refusal) => Over::Refused(refusal),
                        _ => Over::Answered,
                    });
                };
                *self.as_mut().project().settling = Some(savepoint.settle(keep, then));
                continue;
            };
            // Let go as its retry is decided: the wait holds nothing of it — a
            // response's connection, or its stream.
            call.as_mut().let_go();
            self.as_mut().back_off(&retry);
            match savepoint {
                Some(savepoint) => {
                    *self.as_mut().project().settling =
                        Some(savepoint.settle(false, Then::Again(retry)));
                }
                None => self.retrying(&retry),
            }
        }
    }

    /// Whether the run's attempt is out past the instant its bound ends at,
    /// its call not read since, and dropped by no policy's timeout around it.
    fn overdue(&self) -> bool {
        self.stage == Stage::Sending
            && !Cut::encloses()
            && matches!(&self.attempt, Some(Attempt::Sent(sent)) if sent.overdue())
    }

    /// The run dropped with `call`: an attempt out past the instant its bound
    /// ends at — an edge cutting its unit at that deadline polled first, a
    /// worker busy — has its call read once more, so an answer that came in
    /// time counts as its run would have read it, and one still to come has
    /// timed out. Any other drop leaves the attempt to what dropped it
    /// ([`Sent`]).
    pub(crate) fn dropped(self: Pin<&mut Self>, call: Pin<&mut (dyn Calling + '_)>) {
        if !self.overdue() {
            return;
        }
        // A panic of the call's is counted as its failure, its payload the unit's to file.
        let _read = self.poll_attempt(call, &mut Context::from_waker(Waker::noop()));
    }

    /// One attempt of the run's call, as [`poll_attempt`](Self::poll_attempt)
    /// polls it, read once more as it is dropped past its bound
    /// ([`dropped`](Self::dropped)).
    #[cfg(feature = "reqwest")]
    pub(crate) fn attempt<'a>(
        self: Pin<&'a mut Self>,
        call: Pin<&'a mut (dyn Calling + Send + 'a)>,
    ) -> Attempted<'a, 'p> {
        Attempted { run: self, call }
    }

    /// What the run holds let go as it is over, though its future is kept:
    /// its slot, and the attempt's bound, whose timer would wake the task once
    /// more.
    fn over(self: Pin<&mut Self>) {
        let mut this = self.project();
        // Counted now, inside the attempt the run is made in: the run ended failing.
        *this.owed = None;
        *this.slot = None;
        *this.shared_slot = None;
        *this.stage = Stage::Answered;
        this.wait.set(Wait::within());
    }
}

#[cfg(feature = "reqwest")]
impl Run<'_> {
    /// The run over at an answer a part of which is still to come, what it
    /// holds of its policy handed to that part: its slot of the bulkhead, its
    /// last attempt in the breaker when the answer was judged a success, and
    /// what that attempt's bound, counted from its send, has left.
    pub(crate) fn hold(self: Pin<&mut Self>) -> crate::held::Held {
        let this = self.project();
        let bound = *this.bound;
        let Some(sent_at) = *this.sent_at else {
            unreachable!("an answer held past its run was sent");
        };
        let policy = this.terms.policy;
        let attempt = match this.attempt.take() {
            Some(Attempt::Outstanding(mut outstanding)) => outstanding
                .0
                .take()
                .map(|sent| crate::held::Attempt::Own(sent.hold(bound))),
            Some(Attempt::Shared(turn)) => Some(crate::held::Attempt::Shared(turn)),
            _ => None,
        };
        let slot = match this.slot.take().zip(policy.bulkhead.as_ref()) {
            Some((slot, bulkhead)) => Some(crate::held::Slot::Own(bulkhead.hold(slot))),
            None => this.shared_slot.take().map(crate::held::Slot::Shared),
        };
        crate::held::Held::new(
            Arc::clone(policy),
            attempt,
            slot,
            (sent_at + bound).saturating_duration_since(Instant::now()),
            bound,
        )
    }

    /// The run over at an answer with nothing still to come: a success its
    /// last attempt left in the breaker counted as it was judged, then its
    /// slot freed, in a run's order.
    pub(crate) fn release_outstanding(self: Pin<&mut Self>) {
        let this = self.project();
        *this.attempt = None;
        *this.slot = None;
        *this.shared_slot = None;
    }
}

/// One attempt of a run's call, polled until it ends, read once more when it
/// is dropped past its bound ([`Run::dropped`]).
#[cfg(feature = "reqwest")]
pub(crate) struct Attempted<'a, 'p> {
    run: Pin<&'a mut Run<'p>>,
    // `Send`, as the middleware's future holding it is.
    call: Pin<&'a mut (dyn Calling + Send + 'a)>,
}

#[cfg(feature = "reqwest")]
impl Future for Attempted<'_, '_> {
    type Output = Ended;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Ended> {
        let this = &mut *self;
        this.run.as_mut().poll_attempt(this.call.as_mut(), cx)
    }
}

#[cfg(feature = "reqwest")]
impl Drop for Attempted<'_, '_> {
    fn drop(&mut self) {
        self.run.as_mut().dropped(self.call.as_mut());
    }
}

#[cfg(all(test, feature = "reqwest"))]
impl<'p> Run<'p> {
    /// A run of a transport's call under `policy`, opened as the middleware
    /// opens one: holding its slot of the bulkhead when it has one.
    pub(crate) async fn begin(
        policy: &'p Arc<Mechanisms>,
    ) -> Result<Pin<Box<Self>>, ResilienceRefusal> {
        let mut run = Box::pin(Self::holding(policy));
        std::future::poll_fn(|cx| run.as_mut().poll_open(cx)).await?;
        Ok(run)
    }
}

/// What an attempt that `ended` so answered in error — none, for an `Ok` —
/// and the wait its dependency asked before a retry, when the run sends its
/// call again after it: a timeout or an answer judged one to send again,
/// never one the run ends on, nor a refusal that sent nothing — a unit out of
/// time's included, which is no attempt given up at its bound.
fn sent_again<'f>(
    ended: &Ended,
    failure: Option<Failure<'f>>,
) -> Option<(Option<Duration>, Option<Failure<'f>>)> {
    match ended {
        Ended::Judged(Verdict::Failed { asked } | Verdict::IgnoredAgain { asked }) => {
            Some((*asked, failure))
        }
        Ended::Judged(Verdict::Again) => Some((None, None)),
        Ended::Judged(Verdict::Succeeded | Verdict::Final | Verdict::Ignored)
        | Ended::Refused(
            ResilienceRefusal::OUT_OF_TIME
            | ResilienceRefusal::Open { .. }
            | ResilienceRefusal::Full
            | ResilienceRefusal::Limited { .. },
        )
        | Ended::Unwound(_) => None,
        Ended::Refused(ResilienceRefusal::TimedOut { .. }) => {
            Some((None, Some(Failure::Said(TIMED_OUT))))
        }
    }
}

pin_project_lite::pin_project! {
    /// A call polled under its run: the one future a call answering `R` is
    /// compiled into, its closure, its future and its answer held in place
    /// `P`, and its answers judged by `J`.
    pub(crate) struct Running<'p, R, P, J>
    where
        R: ResilientAnswer,
        P: Placing,
        P::Made: Future<Output = R::Attempt>,
        J: Reads<R>,
    {
        // Dropped first: the call's future, then its closure, before what the run
        // holds — the probe's turn, the token, the slot.
        #[pin]
        call: Judged<P, J, R>,
        #[pin]
        run: Run<'p>,
    }

    impl<R, P, J> PinnedDrop for Running<'_, R, P, J>
    where
        R: ResilientAnswer,
        P: Placing,
        P::Made: Future<Output = R::Attempt>,
        J: Reads<R>,
    {
        fn drop(this: Pin<&mut Self>) {
            let this = this.project();
            this.run.dropped(this.call);
        }
    }
}

impl<'p, R, P, J> Running<'p, R, P, J>
where
    R: ResilientAnswer,
    P: Placing,
    P::Made: Future<Output = R::Attempt>,
    J: Reads<R>,
{
    /// `call`, held in `place` and judged by `judging`, run by `run`.
    pub(crate) const fn new(place: P, judging: J, run: Run<'p>) -> Self {
        Self {
            call: Judged::new(place, judging),
            run,
        }
    }
}

impl<R, P, J> Future for Running<'_, R, P, J>
where
    R: ResilientAnswer,
    P: Placing,
    P::Made: Future<Output = R::Attempt>,
    J: Reads<R>,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<R> {
        let mut this = self.project();
        let over = ready!(this.run.as_mut().poll_run(this.call.as_mut(), cx));
        // Taken whatever the run ended on: one its savepoint left unsettled is
        // let go here.
        let answer = this.call.as_mut().take();
        let ran = match (over, answer) {
            (Over::Answered, Some(answer)) => answer.map_err(ResilienceError::Failed),
            (Over::Answered, None) => unreachable!("a run over at its call's answer holds it"),
            (Over::Refused(refusal), _) => {
                Err(ResilienceError::refused(refusal, this.run.instance()))
            }
            (Over::Unsettled(cause), _) => Err(ResilienceError::unsettled(cause)),
        };
        this.run.over();
        this.call.release();
        Poll::Ready(R::answered(ran))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fmt;
    use std::marker::PhantomData;
    use std::sync::atomic::AtomicU32;
    use std::task::Waker;

    use nest_rs_config::{Config, ConfigService};
    use nest_rs_core::{Instance, Problem, error_message, instance};
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::call::Cancellable;
    use crate::config::ResilienceConfig;
    use crate::events::{ResilienceEvent, ResilienceEvents};
    use crate::resilient::{every, judging, judging_by_default, none};
    use crate::teller::{
        FALLBACK_ANSWERS, NOT_ATTEMPTED_AGAIN_PAST_CEILING, NOT_ATTEMPTED_AGAIN_PAST_DEADLINE,
        NOT_ATTEMPTED_AGAIN_UNSETTLED, REFUSED_FULL, REFUSED_LIMITED, RETRYING_ANSWERED,
        RETRYING_FAILED, WAITING_FOR_TOKEN,
    };
    use crate::{BreakerState, ResiliencePolicy, TARGET};

    #[instance]
    struct Upstream;

    #[derive(Debug, PartialEq, Eq, thiserror::Error)]
    #[error("the upstream refused")]
    struct Refused;

    /// An error passing on the wait its dependency asked, as a `Problem`.
    #[derive(Debug, thiserror::Error)]
    #[error("the upstream asked to slow down")]
    struct Throttled(#[source] Problem);

    fn policy(config: ResilienceConfig) -> ResiliencePolicy<Upstream> {
        ResiliencePolicy::new(&config).expect("a valid policy")
    }

    /// The policy `config` describes once it is read as the module reads a
    /// base set in code, held to the ranges a deployment's value is.
    fn pinned(config: ResilienceConfig) -> ResiliencePolicy<Upstream> {
        let read = ResilienceConfig::from_env(&ConfigService::with_vars("resilience", []), config)
            .expect("a pin the config takes");
        policy(read)
    }

    /// The verdict of an attempt that failed on [`Refused`], asking `asked`.
    fn failing(asked: Option<Duration>) -> Verdict {
        Verdict::Failed { asked }
    }

    /// What [`Refused`] says, the failure an answer judged by hand fails on.
    const REFUSED: &str = "the upstream refused";

    /// How the answer of a call sent once counts with no predicate of its
    /// own: every `Err` a failure.
    fn judged<T, E: Error + 'static>(answer: &Result<T, E>) -> Verdict {
        crate::call::judged::<Answer<T, E>, _>(&judging_by_default(), false, answer)
    }

    /// What a call answering `Result<T, E>` answers under its policy.
    type Answer<T, E> = Result<T, ResilienceError<E>>;

    /// An answer judged by hand, as a transport judges its own.
    struct ByHand<J>(J);

    impl<T, E, J> Reads<Answer<T, E>> for ByHand<J>
    where
        J: Fn(&Result<T, E>) -> Verdict,
    {
        fn read(&self, answer: &Result<T, E>, _again: bool) -> Verdict {
            (self.0)(answer)
        }

        fn failure<'a>(&self, _answer: &'a Result<T, E>) -> Option<Failure<'a>> {
            Some(Failure::Said(REFUSED))
        }
    }

    /// A run of a transport's call under `policy`, opened, holding its slot of
    /// the bulkhead when it has one, as the middleware's is.
    async fn begin<I: Instance>(
        policy: &ResiliencePolicy<I>,
    ) -> Result<Pin<Box<Run<'_>>>, ResilienceRefusal> {
        Run::begin(&policy.mechanisms).await
    }

    /// What one attempt of a holding run ended on, and the answer its call
    /// kept: an answer judged a success stays in the breaker, outstanding,
    /// for the run to hold or let go.
    type Attempted<T> = (Ended, Option<Result<T, Refused>>);

    /// One attempt of `run`, its call made of `call` and its answer judged by
    /// `judge`: a panic in the call counts as a failure and unwinds on, as the
    /// middleware's does.
    async fn attempt_once<T, F, Fut, J>(
        mut run: Pin<&mut Run<'_>>,
        call: F,
        judge: J,
    ) -> Attempted<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, Refused>>,
        J: Fn(&Result<T, Refused>) -> Verdict,
    {
        let mut call = pin!(Judged::<_, _, Answer<T, Refused>>::new(
            crate::call::Once::<_, crate::call::Uncancelled>::new(call),
            ByHand(judge),
        ));
        let ended = std::future::poll_fn(|cx| run.as_mut().poll_attempt(call.as_mut(), cx)).await;
        if let Ended::Unwound(payload) = ended {
            resume_unwind(payload);
        }
        (ended, call.take())
    }

    /// Whether `run` attempts its call again after an attempt that `ended`
    /// so, its retry told and its backoff waited when so.
    async fn again(mut run: Pin<&mut Run<'_>>, mut ended: Ended) -> bool {
        let Some(retry) = run.as_mut().retry(&mut ended, Some(Failure::Said(REFUSED))) else {
            return false;
        };
        run.retrying(&retry);
        run.as_mut().back_off(&retry);
        std::future::poll_fn(|cx| run.as_mut().poll_back_off(cx)).await;
        true
    }

    /// Retries on, no breaker, no bulkhead: the subject is the retry.
    fn retrying(retries: usize) -> ResiliencePolicy<Upstream> {
        policy(ResilienceConfig {
            retries: Some(retries),
            breaker_window: None,
            ..ResilienceConfig::default()
        })
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_answering_at_once_is_its_own_answer() {
        let answered = retrying(2)
            .run(|| async { Ok::<_, Refused>("charged") })
            .await;
        assert!(matches!(answered, Ok("charged")), "{answered:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_not_declared_idempotent_is_never_attempted_again() {
        let calls = AtomicU32::new(0);
        let answered = retrying(2)
            .run(|| async {
                calls.fetch_add(1, Ordering::Relaxed);
                Err::<(), _>(Refused)
            })
            .await;
        assert!(
            matches!(answered, Err(ResilienceError::Failed(Refused))),
            "{answered:?}"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn an_idempotent_call_is_attempted_again_after_the_backoff_up_to_its_retries() {
        let calls = AtomicU32::new(0);
        let started = tokio::time::Instant::now();
        let answered = retrying(2)
            .run_idempotent(|| async {
                calls.fetch_add(1, Ordering::Relaxed);
                Err::<(), _>(Refused)
            })
            .await;
        assert!(matches!(answered, Err(ResilienceError::Failed(Refused))));
        assert_eq!(calls.load(Ordering::Relaxed), 3, "one call and two retries");
        let waited = started.elapsed();
        let (first, second) = (Duration::from_millis(200), Duration::from_millis(400));
        assert!(
            waited >= (first + second).mul_f64(0.8) && waited <= (first + second).mul_f64(1.2),
            "two backoffs, ±20 %: {waited:?}",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_idempotent_call_answering_on_a_retry_is_its_answer() {
        let calls = AtomicU32::new(0);
        let answered = retrying(2)
            .run_idempotent(|| async {
                match calls.fetch_add(1, Ordering::Relaxed) {
                    0 => Err(Refused),
                    _ => Ok("tracked"),
                }
            })
            .await;
        assert!(matches!(answered, Ok("tracked")), "{answered:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn an_attempt_past_its_timeout_is_given_up_and_its_token_cancelled() {
        let cancelled = CancellationToken::new();
        let seen = cancelled.clone();
        let answered = policy(ResilienceConfig {
            timeout: Duration::from_millis(300),
            retries: None,
            breaker_window: None,
            ..ResilienceConfig::default()
        })
        .run_cancellable(|cancel| async move {
            let spawned = cancel.clone();
            tokio::spawn(async move {
                spawned.cancelled().await;
                seen.cancel();
            });
            std::future::pending::<Result<(), Refused>>().await
        })
        .await;
        assert!(
            matches!(
                answered,
                Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout == Duration::from_millis(300)
            ),
            "{answered:?}"
        );
        tokio::time::timeout(Duration::from_secs(1), cancelled.cancelled())
            .await
            .expect("work the call spawned is told the attempt was given up");
    }

    /// Where a call panics, run once or again: its closure, before it hands its
    /// future back, or that future, while it runs.
    #[derive(Clone, Copy, Debug)]
    enum PanicSite {
        ClosureOnce,
        FutureOnce,
        ClosureAgain,
        FutureAgain,
    }

    /// Whether the run of a call panicking at `site` unwinds out of the run.
    async fn unwinds(policy: &ResiliencePolicy<Upstream>, site: PanicSite) -> bool {
        let making = || -> std::future::Ready<Result<(), Refused>> {
            panic!("the upstream's client panicked before its future")
        };
        let running = || async {
            if true {
                panic!("the upstream's client panicked");
            }
            Ok::<_, Refused>(())
        };
        let unwound = match site {
            PanicSite::ClosureOnce => nest_rs_core::panic::contain(policy.run(making)).await,
            PanicSite::FutureOnce => nest_rs_core::panic::contain(policy.run(running)).await,
            PanicSite::ClosureAgain => {
                nest_rs_core::panic::contain(policy.run_idempotent(making)).await
            }
            PanicSite::FutureAgain => {
                nest_rs_core::panic::contain(policy.run_idempotent(running)).await
            }
        };
        unwound.is_err()
    }

    /// A panic in the call counts as one failure and unwinds on, for the unit
    /// to contain, wherever the call panics and however it is run: one panic
    /// leaves a breaker opening on two failures closed, and a second opens it.
    #[tokio::test(start_paused = true)]
    async fn a_panicking_call_counts_as_a_failure_and_unwinds_on() {
        for site in [
            PanicSite::ClosureOnce,
            PanicSite::FutureOnce,
            PanicSite::ClosureAgain,
            PanicSite::FutureAgain,
        ] {
            let policy = policy(ResilienceConfig {
                retries: Some(1),
                breaker_window: Some(2),
                breaker_failures: 2,
                ..ResilienceConfig::default()
            });
            assert!(
                unwinds(&policy, site).await,
                "{site:?}: the panic unwinds out of the run"
            );
            assert_eq!(
                policy.breaker_state(),
                Some(BreakerState::Closed),
                "{site:?}: one panic is one failure, of the two the breaker opens on"
            );
            assert!(
                unwinds(&policy, site).await,
                "{site:?}: the panic unwinds out of the run"
            );
            assert!(
                matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
                "{site:?}: a second panic is the second failure: {:?}",
                policy.breaker_state()
            );
        }
    }

    /// A client's future answering at once that panics as it is dropped.
    struct PanicsAsDropped;

    impl Future for PanicsAsDropped {
        type Output = Result<(), Refused>;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Ready(Ok(()))
        }
    }

    impl Drop for PanicsAsDropped {
        fn drop(&mut self) {
            panic!("the upstream's client panicked as its future was dropped");
        }
    }

    /// A call's future that answered, then panics as the run drops it, is no
    /// panic of the call's: the future is dropped outside the poll that counts
    /// the call's panics, so the panic unwinds on as it would with no policy,
    /// and the attempt counts for nothing in a breaker one failure opens.
    #[tokio::test(start_paused = true)]
    async fn a_calls_future_panicking_as_it_is_dropped_unwinds_and_counts_no_failure() {
        for again in [false, true] {
            let policy = policy(ResilienceConfig {
                retries: Some(1),
                breaker_window: Some(1),
                breaker_failures: 1,
                ..ResilienceConfig::default()
            });
            let unwound = if again {
                nest_rs_core::panic::contain(policy.run_idempotent(|| PanicsAsDropped)).await
            } else {
                nest_rs_core::panic::contain(policy.run(|| PanicsAsDropped)).await
            };
            assert!(
                unwound.is_err(),
                "again: {again}; the panic unwinds out of the run"
            );
            assert_eq!(
                policy.breaker_state(),
                Some(BreakerState::Closed),
                "again: {again}; no failure counted"
            );
            assert_eq!(
                policy.breaker_stats().map(|stats| stats.outcomes),
                Some(0),
                "again: {again}; nor a success"
            );
        }
    }

    /// Each attempt's token is its own: live while its call runs and after it
    /// answers, so work the call handed on is not stopped by an answer, and
    /// cancelled as the attempt is given up, before its backoff, the retry
    /// handed a fresh one.
    #[tokio::test(start_paused = true)]
    async fn each_attempts_token_lives_past_its_answer_and_is_cancelled_once_it_is_given_up() {
        let timeout = Duration::from_millis(100);
        let policy = policy(ResilienceConfig {
            timeout,
            retries: Some(1),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        let handed = std::cell::RefCell::new(Vec::new());
        let live_while_running = std::cell::RefCell::new(Vec::new());
        let cancelled_at = Arc::new(std::sync::Mutex::new(None));
        let started = tokio::time::Instant::now();
        let answered = policy
            .run_idempotent_cancellable(|cancel: CancellationToken| {
                handed.borrow_mut().push(cancel.clone());
                let first = handed.borrow().len() == 1;
                if first {
                    let (token, at) = (cancel.clone(), Arc::clone(&cancelled_at));
                    tokio::spawn(async move {
                        token.cancelled().await;
                        *at.lock().expect("one writer") = Some(tokio::time::Instant::now());
                    });
                }
                let live_while_running = &live_while_running;
                async move {
                    tokio::task::yield_now().await;
                    live_while_running.borrow_mut().push(!cancel.is_cancelled());
                    if first {
                        std::future::pending::<()>().await;
                    }
                    Ok::<_, Refused>("the retry")
                }
            })
            .await;
        assert!(matches!(answered, Ok("the retry")), "{answered:?}");
        let handed = handed.into_inner();
        assert_eq!(
            live_while_running.take(),
            [true, true],
            "each attempt's token is live while its call runs"
        );
        assert_eq!(
            handed
                .iter()
                .map(CancellationToken::is_cancelled)
                .collect::<Vec<_>>(),
            [true, false],
            "the timed-out attempt's token is cancelled, the retry's lives past its answer"
        );
        assert_eq!(
            *cancelled_at.lock().expect("one writer"),
            Some(started + timeout),
            "the timed-out attempt's token is cancelled as it is given up, before its backoff"
        );

        let mut handed = None;
        let answered = policy
            .run_cancellable(|cancel: CancellationToken| {
                handed = Some(cancel.clone());
                async move {
                    tokio::task::yield_now().await;
                    Ok::<_, Refused>(!cancel.is_cancelled())
                }
            })
            .await;
        assert!(
            matches!(answered, Ok(true)),
            "live while its call runs: {answered:?}"
        );
        assert!(
            handed.is_some_and(|token| !token.is_cancelled()),
            "a call sent once keeps its token past its answer"
        );
    }

    /// A timed-out attempt's future is dropped before its retry is made: a call
    /// counting the futures it made that are still alive finds none when it
    /// makes the next.
    #[tokio::test(start_paused = true)]
    async fn a_timed_out_attempts_future_is_dropped_before_its_retry_is_made() {
        let policy = policy(ResilienceConfig {
            timeout: Duration::from_millis(100),
            retries: Some(1),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        let alive = std::cell::Cell::new(0_u32);
        let seen = std::cell::RefCell::new(Vec::new());
        let answered = policy
            .run_idempotent(|| {
                seen.borrow_mut().push(alive.get());
                let alive = &alive;
                async move {
                    alive.set(alive.get() + 1);
                    let gone = || alive.set(alive.get() - 1);
                    let _alive = OnDrop(&gone);
                    std::future::pending::<Result<(), Refused>>().await
                }
            })
            .await;
        assert!(
            matches!(
                answered,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::TimedOut { .. },
                    ..
                })
            ),
            "{answered:?}"
        );
        assert_eq!(
            seen.into_inner(),
            [0, 0],
            "none of the futures made before is alive when the next is made"
        );
        assert_eq!(alive.get(), 0);
    }

    /// A future answering at once, as a client's hand-written one does, that
    /// says when it is dropped: an `async` block's locals go when it answers,
    /// before the future itself.
    struct SaysItsDrop<'a>(&'a std::cell::Cell<bool>);

    impl Future for SaysItsDrop<'_> {
        type Output = Result<&'static str, Refused>;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Ready(Ok("answered"))
        }
    }

    impl Drop for SaysItsDrop<'_> {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    /// What the run of a call making a [`SaysItsDrop`] answers.
    type Says<'a> =
        Pin<Box<dyn Future<Output = Result<&'static str, ResilienceError<Refused>>> + 'a>>;

    /// A call sent once has its future dropped before its run answers, the run
    /// still held: what the future holds is let go when the answer is read,
    /// whether its closure is held in place or boxed past tokio's threshold.
    #[tokio::test(start_paused = true)]
    async fn a_call_sent_onces_future_is_dropped_before_its_run_answers() {
        let policy = retrying(0);
        for past in [false, true] {
            let dropped = std::cell::Cell::new(false);
            let says = &dropped;
            let buffer = [7_u8; 16 * 1024 + 1];
            let moving = move || {
                std::hint::black_box(&buffer);
                SaysItsDrop(says)
            };
            assert!(size_of_val(&moving) > 16 * 1024, "past the threshold");
            let mut run: Says<'_> = if past {
                Box::pin(policy.run(moving))
            } else {
                Box::pin(policy.run(|| SaysItsDrop(says)))
            };
            let answered = std::future::poll_fn(|cx| run.as_mut().poll(cx)).await;
            assert!(matches!(answered, Ok("answered")), "{answered:?}");
            assert!(
                dropped.get(),
                "past the threshold: {past}; the future was dropped before the run answered"
            );
        }
    }

    /// A run lets its closure go as it answers, before the answer is read and
    /// though its future is kept: a call sent once the policy refused unsent,
    /// and a call made again, which holds its closure across its attempts.
    #[tokio::test(start_paused = true)]
    async fn a_runs_closure_is_dropped_before_its_answer_is_read() {
        for (again, refused) in [(false, false), (false, true), (true, false), (true, true)] {
            let policy = policy(ResilienceConfig {
                retries: None,
                breaker_window: Some(1),
                breaker_failures: 1,
                ..ResilienceConfig::default()
            });
            if refused {
                policy
                    .open_breaker(Duration::from_secs(10))
                    .expect("an open time `breaker_open` takes");
            }
            let dropped = std::cell::Cell::new(false);
            let gone = || dropped.set(true);
            let held = OnDrop(&gone);
            let call = move || {
                let _held = &held;
                async { Ok::<_, Refused>(()) }
            };
            let mut run: Dropped<'_> = if again {
                Box::pin(policy.run_idempotent(call))
            } else {
                Box::pin(policy.run(call))
            };
            let answered = std::future::poll_fn(|cx| run.as_mut().poll(cx)).await;
            assert_eq!(answered.is_err(), refused, "{answered:?}");
            assert!(
                dropped.get(),
                "again: {again}, refused: {refused}; the closure was dropped before the answer \
                 was read"
            );
        }
    }

    /// Counts the wakes of a task a test polls by hand.
    struct CountsWakes(AtomicU32);

    impl std::task::Wake for CountsWakes {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A run lets its attempt's bound go as it answers: kept unread past the
    /// instant that bound would have ended at, it wakes its task no more.
    #[tokio::test(start_paused = true)]
    async fn a_run_kept_after_it_answers_is_woken_no_more_once_its_attempts_bound_passes() {
        for again in [false, true] {
            let policy = policy(ResilienceConfig {
                timeout: Duration::from_secs(5),
                retries: None,
                breaker_window: None,
                ..ResilienceConfig::default()
            });
            let gate = tokio::sync::Notify::new();
            let call = || async {
                gate.notified().await;
                Ok::<_, Refused>(())
            };
            let mut run: Dropped<'_> = if again {
                Box::pin(policy.run_idempotent(call))
            } else {
                Box::pin(policy.run(call))
            };
            let wakes = Arc::new(CountsWakes(AtomicU32::new(0)));
            let waker = std::task::Waker::from(Arc::clone(&wakes));
            let mut cx = Context::from_waker(&waker);
            assert!(
                run.as_mut().poll(&mut cx).is_pending(),
                "its call waits, its bound's timer armed"
            );
            gate.notify_one();
            let answered = run.as_mut().poll(&mut cx);
            assert!(matches!(answered, Poll::Ready(Ok(()))), "{answered:?}");
            let woken = wakes.0.load(Ordering::Relaxed);
            tokio::time::advance(Duration::from_secs(6)).await;
            tokio::task::yield_now().await;
            assert_eq!(
                wakes.0.load(Ordering::Relaxed),
                woken,
                "again: {again}; no wake once the run answered"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_timed_out_idempotent_call_is_attempted_again() {
        let calls = AtomicU32::new(0);
        let answered = policy(ResilienceConfig {
            timeout: Duration::from_millis(100),
            retries: Some(1),
            breaker_window: None,
            ..ResilienceConfig::default()
        })
        .run_idempotent(|| async {
            if calls.fetch_add(1, Ordering::Relaxed) == 0 {
                std::future::pending::<()>().await;
            }
            Ok::<_, Refused>("second")
        })
        .await;
        assert!(matches!(answered, Ok("second")), "{answered:?}");
    }

    /// The unit's deadline, ambient, bounds every attempt before the policy's
    /// own timeout does, and an attempt it cut counts in the breaker.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_ends_by_the_units_deadline_and_counts_as_timed_out() {
        let policy = policy(ResilienceConfig {
            timeout: Duration::from_secs(5),
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            ..ResilienceConfig::default()
        });
        let left = Duration::from_millis(300);
        let started = tokio::time::Instant::now();
        let given_up = nest_rs_core::__private::with_deadline(
            Some(started + left),
            policy.run(std::future::pending::<Result<(), Refused>>),
        )
        .await;
        assert!(
            matches!(given_up, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout == left),
            "{given_up:?}"
        );
        assert_eq!(started.elapsed(), left, "the deadline, not the 5 s timeout");
        let refused = policy.run(|| async { Ok::<_, Refused>(()) }).await;
        assert!(
            matches!(
                refused,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                })
            ),
            "the cut attempt opened the breaker: {refused:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn no_retry_starts_whose_wait_ends_past_the_units_deadline_nor_any_call_with_none_left() {
        let calls = AtomicU32::new(0);
        let failing = || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Err::<(), _>(Refused)
        };
        let policy = retrying(2);
        let now = tokio::time::Instant::now();
        let answered = nest_rs_core::__private::with_deadline(
            Some(now + Duration::from_millis(100)),
            policy.run_idempotent(&failing),
        )
        .await;
        assert!(matches!(answered, Err(ResilienceError::Failed(Refused))));
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "the first retry waits about 200 ms, past the 100 ms left"
        );

        let spent = nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now()),
            policy.run_idempotent(failing),
        )
        .await;
        assert!(
            matches!(spent, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout.is_zero()),
            "{spent:?}"
        );
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "a unit out of time sends nothing"
        );
    }

    /// A retry starts only when its backoff ends before the unit's deadline,
    /// but the backoff's timer may fire past it: the retry is then refused as
    /// a unit out of time, unsent.
    #[tokio::test(start_paused = true)]
    async fn a_retry_whose_backoff_ends_past_the_units_deadline_is_refused_unsent() {
        let calls = AtomicU32::new(0);
        let policy = retrying(1);
        let mut failing = Box::pin(nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now() + Duration::from_secs(1)),
            policy.run_idempotent(|| async {
                calls.fetch_add(1, Ordering::Relaxed);
                Err::<(), _>(Refused)
            }),
        ));
        park(&mut failing).await;
        tokio::time::advance(Duration::from_secs(2)).await;
        let late = failing.await;
        assert!(
            matches!(late, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout.is_zero()),
            "a backoff of about 200 ms ended at 2 s, past the deadline at 1 s: {late:?}"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1, "the retry is never sent");
    }

    /// Nothing left of the unit's deadline refuses before the breaker is asked:
    /// a unit out of time meeting an open breaker is told it is out of time.
    #[tokio::test(start_paused = true)]
    async fn nothing_left_of_the_units_deadline_refuses_before_an_open_breaker() {
        let policy = policy(ResilienceConfig {
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            ..ResilienceConfig::default()
        });
        let failed = policy.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        let spent = nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now()),
            policy.run(answering),
        )
        .await;
        assert!(
            matches!(spent, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout.is_zero()),
            "{spent:?}"
        );
        let open = policy.run(answering).await;
        assert!(
            matches!(
                open,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                })
            ),
            "the breaker the failure opened: {open:?}"
        );
    }

    /// A wait the call's error asks, through a `Problem` in its chain, is waited
    /// whole before the retry, stretched by a fifth at most.
    #[tokio::test(start_paused = true)]
    async fn a_wait_the_calls_error_asks_is_honoured_before_its_retry() {
        let calls = AtomicU32::new(0);
        let started = tokio::time::Instant::now();
        let answered = retrying(1)
            .run_idempotent(|| async {
                match calls.fetch_add(1, Ordering::Relaxed) {
                    0 => Err(Throttled(
                        Problem::new(429, nest_rs_core::code::RATE_LIMITED).with_retry_after(1),
                    )),
                    _ => Ok("answered"),
                }
            })
            .await;
        assert!(matches!(answered, Ok("answered")), "{answered:?}");
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_secs(1) && waited <= Duration::from_millis(1_200),
            "{waited:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_open_breaker_refuses_at_once_naming_its_wait_and_ends_the_run() {
        let calls = AtomicU32::new(0);
        let policy = policy(ResilienceConfig {
            retries: Some(5),
            breaker_window: Some(2),
            breaker_failures: 2,
            breaker_open: Duration::from_secs(30),
            ..ResilienceConfig::default()
        });
        let answered = policy
            .run_idempotent(|| async {
                calls.fetch_add(1, Ordering::Relaxed);
                Err::<(), _>(Refused)
            })
            .await;
        assert!(
            matches!(
                answered,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                })
            ),
            "the breaker opened under the run: {answered:?}"
        );
        assert_eq!(
            calls.load(Ordering::Relaxed),
            2,
            "no attempt past the one that opened it"
        );

        let refused = policy
            .run(|| async {
                calls.fetch_add(1, Ordering::Relaxed);
                Ok::<_, Refused>(())
            })
            .await;
        let Err(ResilienceError::Refused {
            refusal: ResilienceRefusal::Open { retry_after, .. },
            ..
        }) = refused
        else {
            panic!("refused while open: {refused:?}");
        };
        assert!(retry_after <= Duration::from_secs(30), "{retry_after:?}");
        assert_eq!(
            calls.load(Ordering::Relaxed),
            2,
            "a refused call is never sent"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_full_bulkhead_refuses_the_call_beyond_its_slots() {
        let policy = Arc::new(policy(ResilienceConfig {
            concurrency: Some(1),
            breaker_window: None,
            ..ResilienceConfig::default()
        }));
        let release = Arc::new(tokio::sync::Notify::new());
        let holding = {
            let (policy, release) = (Arc::clone(&policy), Arc::clone(&release));
            tokio::spawn(async move {
                policy
                    .run(|| async move {
                        release.notified().await;
                        Ok::<_, Refused>(())
                    })
                    .await
            })
        };
        tokio::task::yield_now().await;
        let refused = policy.run(|| async { Ok::<_, Refused>(()) }).await;
        assert!(
            matches!(
                refused,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Full,
                    ..
                })
            ),
            "{refused:?}"
        );
        release.notify_one();
        assert!(holding.await.expect("the held call ends").is_ok());
        assert!(policy.run(|| async { Ok::<_, Refused>(()) }).await.is_ok());
    }

    /// A call meeting an open breaker is refused before it waits for a slot of
    /// the full bulkhead, however long that wait would be.
    #[tokio::test(start_paused = true)]
    async fn an_open_breaker_refuses_before_any_wait_for_a_slot() {
        let policy = policy(ResilienceConfig {
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(60)),
            ..ResilienceConfig::default()
        });
        let held = begin(&policy).await.expect("the one slot, while closed");
        policy
            .mechanisms
            .breaker
            .as_ref()
            .expect("a breaker")
            .admit(policy.mechanisms.timeout)
            .expect("closed")
            .send(policy.mechanisms.timeout, None)
            .settle(Some(TIMED_OUT));

        let started = tokio::time::Instant::now();
        let refused = policy.run(|| async { Ok::<_, Refused>(()) }).await;
        assert!(
            matches!(
                refused,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                })
            ),
            "{refused:?}"
        );
        assert_eq!(
            started.elapsed(),
            Duration::ZERO,
            "no wait for the slot held"
        );
        drop(held);
    }

    /// A run with a bulkhead asks the unit's deadline first too, before the
    /// breaker and the slot: a unit out of time is told so whatever the
    /// policy's slots, its refusal the same with a bulkhead as without one.
    #[tokio::test(start_paused = true)]
    async fn nothing_left_of_the_units_deadline_refuses_before_the_bulkhead_and_its_breaker() {
        let policy = policy(ResilienceConfig {
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            concurrency: Some(1),
            ..ResilienceConfig::default()
        });
        let spent = || {
            nest_rs_core::__private::with_deadline(
                Some(tokio::time::Instant::now()),
                policy.run(answering),
            )
        };
        let held = begin(&policy).await.expect("the one slot");
        let refused = spent().await;
        assert!(
            matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout.is_zero()),
            "out of time before the full bulkhead: {refused:?}"
        );
        drop(held);

        let failed = policy.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        let refused = spent().await;
        assert!(
            matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout.is_zero()),
            "out of time before the open breaker: {refused:?}"
        );
    }

    /// A call waits for a slot within the unit's deadline, but a slot freed
    /// for it may reach it past that deadline, its task run late: it is then
    /// refused as a unit out of time, unsent, and the slot freed again.
    #[tokio::test(start_paused = true)]
    async fn a_call_handed_its_slot_past_the_units_deadline_is_refused_unsent() {
        let policy = policy(ResilienceConfig {
            retries: None,
            breaker_window: None,
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(10)),
            ..ResilienceConfig::default()
        });
        let held = begin(&policy).await.expect("the one slot");
        let sent = AtomicU32::new(0);
        let mut waiting = Box::pin(nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now() + Duration::from_secs(5)),
            policy.run(|| async {
                sent.fetch_add(1, Ordering::Relaxed);
                Ok::<_, Refused>(())
            }),
        ));
        park(&mut waiting).await;
        drop(held);
        tokio::time::advance(Duration::from_secs(6)).await;
        let late = waiting.await;
        assert!(
            matches!(late, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout.is_zero()),
            "the slot freed for it, its task run at 6 s, past its deadline at 5 s: {late:?}"
        );
        assert_eq!(
            sent.load(Ordering::Relaxed),
            0,
            "a unit out of time sends nothing"
        );
        assert!(
            policy.run(answering).await.is_ok(),
            "the slot it was handed is free again"
        );
    }

    /// More callers at once than slots and waiters: the slots' holders are
    /// sent, exactly `concurrency_waiters` wait for a slot and are sent in
    /// turn, and every other caller is refused `Full` at the same instant,
    /// unsent.
    #[tokio::test(start_paused = true)]
    async fn more_callers_than_slots_and_waiters_leave_exactly_the_waiters_waiting() {
        let policy = Arc::new(policy(ResilienceConfig {
            retries: None,
            breaker_window: None,
            concurrency: Some(2),
            concurrency_wait: Some(Duration::from_secs(10)),
            concurrency_waiters: Some(3),
            ..ResilienceConfig::default()
        }));
        let started = tokio::time::Instant::now();
        let callers: Vec<_> = (0..12)
            .map(|_| {
                let policy = Arc::clone(&policy);
                tokio::spawn(async move {
                    let answered = policy
                        .run(|| async {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            Ok::<_, Refused>(())
                        })
                        .await;
                    (started.elapsed(), answered)
                })
            })
            .collect();
        let (mut sent, mut refused) = (Vec::new(), Vec::new());
        for caller in callers {
            match caller.await.expect("the caller's task") {
                (at, Ok(())) => sent.push(at.as_secs()),
                (
                    at,
                    Err(ResilienceError::Refused {
                        refusal: ResilienceRefusal::Full,
                        ..
                    }),
                ) => refused.push(at),
                (_, other) => panic!("sent, or refused for a full bulkhead: {other:?}"),
            }
        }
        sent.sort_unstable();
        assert_eq!(
            sent,
            [1, 1, 2, 2, 3],
            "the two holding the slots answer at 1 s, then the three waiting, in turn"
        );
        assert_eq!(
            refused,
            [Duration::ZERO; 7],
            "every other caller is refused at once"
        );
    }

    /// A call finding every slot taken and as many calls waiting as may is
    /// refused at once, filing the full bulkhead's line with the waiters'
    /// bound it met; a call whose wait ran out files the line without it.
    /// From a decorated method, each names its host and its method.
    #[tokio::test(start_paused = true)]
    async fn a_call_the_waiters_bound_refuses_files_the_full_bulkheads_line_naming_the_bound() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(ResilienceConfig {
            retries: None,
            breaker_window: None,
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(1)),
            concurrency_waiters: Some(1),
            ..ResilienceConfig::default()
        });
        const TRACKING: ResilientMethod =
            crate::resilient::resilient_method(|| "ShipmentsService", "track", None, None);
        let held = begin(&policy).await.expect("the one slot");
        let started = tokio::time::Instant::now();
        let (waited, crowded) = tokio::join!(
            policy.run_once::<Ran, _, Cancellable, _>(
                Some(&TRACKING),
                judging_by_default(),
                |_| answering()
            ),
            async {
                tokio::task::yield_now().await;
                let crowded = policy
                    .run_once::<Ran, _, Cancellable, _>(
                        Some(&TRACKING),
                        judging_by_default(),
                        |_| answering(),
                    )
                    .await;
                (started.elapsed(), crowded)
            },
        );
        drop(held);
        assert!(
            matches!(
                waited,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Full,
                    ..
                })
            ),
            "its wait ran out: {waited:?}"
        );
        assert!(
            matches!(crowded, (at, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Full, .. })) if at.is_zero()),
            "refused at once, the one place taken: {crowded:?}"
        );

        let full = lines.find(TARGET, REFUSED_FULL);
        assert_eq!(full.len(), 2, "{full:#?}");
        for line in &full {
            assert_eq!(line.level, "debug", "{line:?}");
            assert_eq!(line.field("instance").as_deref(), Some("upstream"));
            assert_eq!(line.field("host").as_deref(), Some("ShipmentsService"));
            assert_eq!(line.field("method").as_deref(), Some("track"));
        }
        assert_eq!(
            full[0].field("waiters").as_deref(),
            Some("1"),
            "the call refused at once"
        );
        assert_eq!(
            full[1].field("waiters"),
            None,
            "the call whose wait ran out"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_asked_wait_is_honoured_up_to_the_ceiling_and_past_it_ends_the_run() {
        let policy = retrying(3);
        let mut run = begin(&policy).await.expect("a run");
        let asked = Duration::from_secs(1);
        let started = tokio::time::Instant::now();
        let failed = Ended::Judged(failing(Some(asked)));
        assert!(again(run.as_mut(), failed).await);
        let waited = started.elapsed();
        assert!(
            waited >= asked && waited <= asked.mul_f64(1.2),
            "the ask, stretched by a fifth at most: {waited:?}"
        );
        let too_long = Ended::Judged(failing(Some(Duration::from_secs(3))));
        assert!(!again(run.as_mut(), too_long).await, "past the 2 s ceiling");
    }

    /// A backoff at its floor of a millisecond waits from 0.8 ms to 1.2 ms, its
    /// jitter's, and each wait a retry's lines file is in whole milliseconds
    /// rounded up: one drawn under a millisecond is `1`, never the `0` of no
    /// wait, as the retry's wait and as the wait no time is left for, and a
    /// ceiling of a millisecond and a half is `2` beside a wait asked past it.
    /// The retry's wait ends no sooner than drawn.
    #[tokio::test(start_paused = true)]
    async fn a_backoff_drawn_under_a_millisecond_is_filed_as_one_and_each_retry_wait_rounds_up() {
        let lines = nest_rs_testing::LogCapture::install();
        let floor = Duration::from_millis(1);
        let policy = pinned(ResilienceConfig {
            retries: Some(1),
            backoff_first: floor,
            backoff_ceiling: Duration::from_micros(1_500),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        let seed = (0..)
            .find(|seed| policy.mechanisms.backoff.wait(NonZeroU32::MIN, *seed) < floor)
            .expect("a seed drawing the jitter's lower half");
        let drawn = policy.mechanisms.backoff.wait(NonZeroU32::MIN, seed);
        let run = || async {
            let mut run = begin(&policy).await.expect("a run");
            *run.as_mut().project().seed = Some(seed);
            run
        };
        let failed = || Ended::Judged(failing(None));

        let mut retrying = run().await;
        let started = tokio::time::Instant::now();
        assert!(again(retrying.as_mut(), failed()).await, "attempted again");
        assert!(
            started.elapsed() >= drawn,
            "{:?}, drawn {drawn:?}",
            started.elapsed()
        );

        let ended = nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now() + drawn / 2),
            async {
                let mut run = run().await;
                again(run.as_mut(), failed()).await
            },
        )
        .await;
        assert!(!ended, "the unit's deadline comes before the wait ends");

        let asked = Ended::Judged(failing(Some(Duration::from_secs(3))));
        let mut asking = run().await;
        assert!(!again(asking.as_mut(), asked).await, "past the ceiling");

        let retried = lines.expect_one(TARGET, RETRYING_FAILED);
        let out_of_time = lines.expect_one(TARGET, NOT_ATTEMPTED_AGAIN_PAST_DEADLINE);
        for line in [&retried, &out_of_time] {
            assert_eq!(line.field("wait_ms").as_deref(), Some("1"), "{line:?}");
        }
        let past = lines.expect_one(TARGET, NOT_ATTEMPTED_AGAIN_PAST_CEILING);
        assert_eq!(
            (past.field("asked_ms"), past.field("ceiling_ms")),
            (Some("3000".to_owned()), Some("2".to_owned())),
            "{past:?}"
        );
    }

    /// No retry and no breaker, a bucket of `limit` tokens refilled over
    /// `window`, each attempt finding none free waiting up to `wait` for the
    /// next, or refused at once with none: the subject is the rate.
    fn rated(limit: usize, window: Duration, wait: Option<Duration>) -> ResilienceConfig {
        ResilienceConfig {
            retries: None,
            breaker_window: None,
            rate_limit: Some(limit),
            rate_window: window,
            rate_wait: wait,
            ..ResilienceConfig::default()
        }
    }

    /// What a call the test does not count answers.
    async fn answering() -> Result<(), Refused> {
        Ok(())
    }

    /// What the run of [`answering`], or of any call answering as it does,
    /// answers.
    type Ran = Result<(), ResilienceError<Refused>>;

    /// Polls `run` once, into the wait it parks in. Moving the clock past that
    /// wait's end before `run` is polled again wakes it late, as a busy runtime
    /// does, which a paused clock, waking each sleeper at its instant, never
    /// does on its own.
    async fn park<F>(run: &mut std::pin::Pin<Box<F>>)
    where
        F: Future + ?Sized,
        F::Output: fmt::Debug,
    {
        tokio::select! {
            biased;
            answered = run.as_mut() => panic!("it parks in its wait: {answered:?}"),
            () = std::future::ready(()) => {}
        }
    }

    #[tokio::test(start_paused = true)]
    async fn every_attempt_sent_takes_a_token_of_the_rate_a_retry_included() {
        let policy = policy(ResilienceConfig {
            retries: Some(1),
            ..rated(3, Duration::from_secs(60), None)
        });
        let calls = AtomicU32::new(0);
        let sent = || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Refused>(())
        };
        assert!(policy.run(&sent).await.is_ok());
        let retried = policy
            .run_idempotent(|| async {
                match calls.fetch_add(1, Ordering::Relaxed) {
                    1 => Err(Refused),
                    _ => Ok(()),
                }
            })
            .await;
        assert!(matches!(retried, Ok(())), "{retried:?}");
        assert_eq!(
            calls.load(Ordering::Relaxed),
            3,
            "a call, then one failing and its retry"
        );
        let refused = policy.run(sent).await;
        assert!(
            matches!(
                refused,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Limited { .. },
                    ..
                })
            ),
            "the retry took the bucket's last token: {refused:?}"
        );
        assert_eq!(
            calls.load(Ordering::Relaxed),
            3,
            "a refused call is never sent"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn past_the_limit_a_call_is_refused_unsent_told_the_time_to_the_next_token() {
        let policy = policy(rated(2, Duration::from_secs(60), None));
        let calls = AtomicU32::new(0);
        let sent = || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Refused>(())
        };
        assert!(policy.run(&sent).await.is_ok());
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(policy.run(&sent).await.is_ok());
        let started = tokio::time::Instant::now();
        let Err(refused) = policy.run(sent).await else {
            panic!("past the limit");
        };
        let ResilienceError::Refused {
            refusal: ResilienceRefusal::Limited { retry_after, .. },
            ..
        } = &refused
        else {
            panic!("refused by the rate: {refused:?}");
        };
        assert_eq!(
            *retry_after,
            Duration::from_secs(20),
            "the token the first call took is back half a window after it, at 30 s"
        );
        assert_eq!(started.elapsed(), Duration::ZERO, "refused at once");
        let problem = nest_rs_core::__private::find_in_chain::<Problem>(&refused)
            .expect("the problem rides the chain");
        assert_eq!(
            (problem.status(), problem.code(), problem.retry_after()),
            (503, nest_rs_core::code::UNAVAILABLE, Some(20))
        );
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    /// Nothing left of the unit's deadline refuses before the breaker and the
    /// rate are asked: the attempt takes no token, and is told as a unit out
    /// of time while no token is free either.
    #[tokio::test(start_paused = true)]
    async fn nothing_left_of_the_units_deadline_refuses_before_the_rate_is_asked() {
        let policy = policy(rated(1, Duration::from_secs(60), None));
        let spent = || {
            nest_rs_core::__private::with_deadline(
                Some(tokio::time::Instant::now()),
                policy.run(answering),
            )
        };
        let refused = spent().await;
        assert!(
            matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout.is_zero()),
            "{refused:?}"
        );
        assert!(
            policy.run(answering).await.is_ok(),
            "the attempt out of time took no token"
        );
        let refused = spent().await;
        assert!(
            matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout.is_zero()),
            "no time left is said before the rate's limit: {refused:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_an_open_breaker_refuses_takes_no_token_of_the_rate() {
        let policy = policy(ResilienceConfig {
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(10),
            ..rated(2, Duration::from_secs(60), None)
        });
        let failed = policy.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        for _ in 0..3 {
            let refused = policy.run(answering).await;
            assert!(
                matches!(
                    refused,
                    Err(ResilienceError::Refused {
                        refusal: ResilienceRefusal::Open { .. },
                        ..
                    })
                ),
                "{refused:?}"
            );
        }
        tokio::time::advance(Duration::from_secs(10)).await;
        let probed = policy.run(answering).await;
        assert!(
            matches!(probed, Ok(())),
            "the probe takes the bucket's second token: {probed:?}"
        );
    }

    /// An attempt the rate refuses was not sent, so a closed breaker counts it
    /// as no failure.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_the_rate_refuses_settles_nothing_in_a_closed_breaker() {
        let policy = policy(ResilienceConfig {
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(3600),
            ..rated(1, Duration::from_secs(60), None)
        });
        assert!(policy.run(answering).await.is_ok());
        let limited = policy.run(answering).await;
        assert!(
            matches!(
                limited,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Limited { .. },
                    ..
                })
            ),
            "{limited:?}"
        );
        tokio::time::advance(Duration::from_secs(60)).await;
        let answered = policy.run(answering).await;
        assert!(
            matches!(answered, Ok(())),
            "the breaker stayed closed: {answered:?}"
        );
    }

    /// A probe the rate refuses was not sent: it neither closes the breaker
    /// nor opens it again, and the next call probes in its turn.
    #[tokio::test(start_paused = true)]
    async fn a_probe_the_rate_refuses_settles_nothing_and_hands_its_turn_to_the_next_call() {
        let policy = policy(ResilienceConfig {
            breaker_window: Some(2),
            breaker_failures: 2,
            breaker_open: Duration::from_secs(30),
            ..rated(2, Duration::from_secs(120), None)
        });
        for _ in 0..2 {
            let failed = policy.run(|| async { Err::<(), _>(Refused) }).await;
            assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        }
        tokio::time::advance(Duration::from_secs(30)).await;
        for nth in ["the probe", "the call after it, probing in its turn"] {
            let refused = policy.run(answering).await;
            assert!(
                matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Limited { retry_after, .. }, .. })
                    if retry_after == Duration::from_secs(30)),
                "{nth}, admitted by the breaker and refused by the rate: {refused:?}"
            );
        }
        tokio::time::advance(Duration::from_secs(30)).await;
        let failed = policy.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(
            matches!(failed, Err(ResilienceError::Failed(Refused))),
            "a token is back, and a probe is sent at last: {failed:?}"
        );
        let reopened = policy.run(answering).await;
        assert!(
            matches!(
                reopened,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                })
            ),
            "the failed probe opened the breaker again, which no refused probe closed: {reopened:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_attempt_finding_no_token_free_waits_for_the_next_its_wait_reaches_then_is_sent() {
        let policy = policy(rated(
            1,
            Duration::from_secs(10),
            Some(Duration::from_secs(10)),
        ));
        let calls = AtomicU32::new(0);
        let sent = || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Refused>(())
        };
        assert!(policy.run(&sent).await.is_ok());
        tokio::time::advance(Duration::from_secs(4)).await;
        let started = tokio::time::Instant::now();
        let answered = policy.run(sent).await;
        assert!(matches!(answered, Ok(())), "{answered:?}");
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(6),
            "until its token, at 10 s"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_token_past_the_wait_or_at_the_units_deadline_is_not_waited_for() {
        let policy = policy(rated(
            1,
            Duration::from_secs(10),
            Some(Duration::from_secs(5)),
        ));
        assert!(policy.run(answering).await.is_ok());
        tokio::time::advance(Duration::from_secs(4)).await;
        let started = tokio::time::Instant::now();
        let refused = policy.run(answering).await;
        assert!(
            matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Limited { retry_after, .. }, .. })
                if retry_after == Duration::from_secs(6)),
            "a token 6 s away, past the 5 s wait: {refused:?}"
        );
        assert_eq!(started.elapsed(), Duration::ZERO, "nothing waited");

        tokio::time::advance(Duration::from_secs(2)).await;
        let deadline = |left| Some(tokio::time::Instant::now() + left);
        let refused = nest_rs_core::__private::with_deadline(
            deadline(Duration::from_secs(4)),
            policy.run(answering),
        )
        .await;
        assert!(
            matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Limited { retry_after, .. }, .. })
                if retry_after == Duration::from_secs(4)),
            "a token at the unit's deadline leaves its call no time: {refused:?}"
        );
        assert_eq!(started.elapsed(), Duration::from_secs(2), "nothing waited");

        let answered = nest_rs_core::__private::with_deadline(
            deadline(Duration::from_secs(5)),
            policy.run(answering),
        )
        .await;
        assert!(matches!(answered, Ok(())), "{answered:?}");
        assert_eq!(started.elapsed(), Duration::from_secs(6));
    }

    /// A bucket left to fill sends its limit at once, then a token at a time:
    /// two per 10 s send two at once, a third 5 s on and a fourth at 10 s,
    /// twice the limit within one window's length.
    #[tokio::test(start_paused = true)]
    async fn a_bucket_left_to_fill_sends_its_limit_at_once_then_a_token_at_a_time() {
        let window = Duration::from_secs(10);
        let policy = policy(rated(2, window, Some(window)));
        tokio::time::advance(window * 3).await;
        let started = tokio::time::Instant::now();
        let mut sent = Vec::new();
        for _ in 0..4 {
            assert!(policy.run(answering).await.is_ok());
            sent.push(started.elapsed().as_secs());
        }
        assert_eq!(sent, [0, 0, 5, 10]);
    }

    /// A retry takes a token as a first attempt does, and waits for one in
    /// its turn: the one token taken by the attempt that failed, its retry is
    /// sent once the next is back, past its backoff.
    #[tokio::test(start_paused = true)]
    async fn a_retry_finding_no_token_free_waits_for_the_next_as_a_first_attempt_does() {
        let window = Duration::from_secs(10);
        let policy = policy(ResilienceConfig {
            retries: Some(1),
            ..rated(1, window, Some(window))
        });
        let calls = AtomicU32::new(0);
        let started = tokio::time::Instant::now();
        let answered = policy
            .run_idempotent(|| async {
                match calls.fetch_add(1, Ordering::Relaxed) {
                    0 => Err(Refused),
                    _ => Ok(started.elapsed()),
                }
            })
            .await;
        assert!(
            matches!(answered, Ok(sent) if sent == window),
            "its retry sent at 10 s, the token its first attempt took back: {answered:?}"
        );
    }

    /// An attempt holding its call, as the middleware's does, takes a token as
    /// any other: finding none free it waits for the next, and one whose token
    /// lies past the rate's wait is refused unsent, told the time to it.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_holding_its_call_takes_a_token_and_waits_for_one_as_any_other() {
        let window = Duration::from_secs(10);
        let policy = policy(rated(1, window, Some(window)));
        let sent = AtomicU32::new(0);
        let sending = || async {
            sent.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Refused>(())
        };
        let succeeded = |_: &Result<(), Refused>| Verdict::Succeeded;
        let started = tokio::time::Instant::now();
        for at in [Duration::ZERO, window] {
            let mut run = begin(&policy).await.expect("a run");
            let attempted = attempt_once(run.as_mut(), sending, succeeded).await;
            assert!(matches!(
                attempted,
                (Ended::Judged(Verdict::Succeeded), Some(Ok(())))
            ));
            assert_eq!(started.elapsed(), at, "the second waited for its token");
        }
        let mut first = begin(&policy).await.expect("a run");
        let mut second = begin(&policy).await.expect("a run");
        let ((waited, _), (refused, _)) = tokio::join!(
            attempt_once(first.as_mut(), sending, succeeded),
            attempt_once(second.as_mut(), sending, succeeded),
        );
        assert!(matches!(waited, Ended::Judged(Verdict::Succeeded)));
        assert!(
            matches!(refused, Ended::Refused(ResilienceRefusal::Limited { retry_after: wait }) if wait == window * 2),
            "due the token after the one waited for, 20 s away"
        );
        assert_eq!(started.elapsed(), window * 2);
        assert_eq!(sent.load(Ordering::Relaxed), 3, "the one refused unsent");
    }

    /// Two attempts find no token free: the first reserves the next one, 10 s
    /// away, and the second is due the one after, 20 s away. A wait that
    /// reaches it sends it there, after the one wait it was told; a shorter
    /// one refuses it at once, told those 20 s, rather than after a first
    /// wait: no attempt's waits add up past the rate's wait.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_waits_once_for_the_token_it_is_due_behind_the_attempts_before_it() {
        for (wait, behind) in [(25, "sent at 20s"), (15, "limited 20s at 0ns")] {
            let policy = policy(rated(
                1,
                Duration::from_secs(10),
                Some(Duration::from_secs(wait)),
            ));
            assert!(policy.run(answering).await.is_ok());
            let started = tokio::time::Instant::now();
            let answered = |answered: Ran| match answered {
                Ok(()) => format!("sent at {:?}", started.elapsed()),
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Limited { retry_after, .. },
                    ..
                }) => {
                    format!("limited {retry_after:?} at {:?}", started.elapsed())
                }
                Err(other) => panic!("{other:?}"),
            };
            let (first, second) =
                tokio::join!(async { answered(policy.run(answering).await) }, async {
                    answered(policy.run(answering).await)
                },);
            assert_eq!(
                (first.as_str(), second.as_str()),
                ("sent at 10s", behind),
                "with {wait} s to wait"
            );
        }
    }

    /// An attempt woken late for its token still has it: parked at 0 s for the
    /// token at 10 s and polled at 12 s, it is sent at once, and a call that
    /// arrived at 12 s, before it was polled, took nothing of it, due the
    /// token after, at 20 s.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_woken_late_for_its_token_is_sent_ahead_of_a_call_arrived_since() {
        let policy = policy(rated(
            1,
            Duration::from_secs(10),
            Some(Duration::from_secs(10)),
        ));
        assert!(policy.run(answering).await.is_ok());
        let started = tokio::time::Instant::now();
        let mut waiting = Box::pin(policy.run(answering));
        park(&mut waiting).await;
        tokio::time::advance(Duration::from_secs(12)).await;
        let mut arrived = Box::pin(policy.run(answering));
        park(&mut arrived).await;
        assert!(waiting.await.is_ok());
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(12),
            "sent as soon as it is polled"
        );
        assert!(arrived.await.is_ok());
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(20),
            "at the token after"
        );
    }

    /// A call dropped while it waits for its token gives it to the attempt
    /// behind it, woken where it waits: due the token at 20 s, that one is
    /// sent at 10 s, the instant the dropped call's token is back.
    #[tokio::test(start_paused = true)]
    async fn a_call_dropped_while_it_waits_gives_its_token_to_the_attempt_behind_it() {
        let policy = Arc::new(policy(rated(
            1,
            Duration::from_secs(10),
            Some(Duration::from_secs(20)),
        )));
        assert!(policy.run(answering).await.is_ok());
        let started = tokio::time::Instant::now();
        let mut first = Box::pin(policy.run(answering));
        park(&mut first).await;
        let behind = tokio::spawn({
            let policy = Arc::clone(&policy);
            async move {
                let answered = policy.run(answering).await;
                (answered, started.elapsed())
            }
        });
        tokio::time::sleep(Duration::from_secs(4)).await;
        drop(first);
        let (answered, at) = behind.await.expect("the call's task");
        assert!(answered.is_ok(), "{answered:?}");
        assert_eq!(
            at,
            Duration::from_secs(10),
            "woken by the token given back, 10 s sooner than it was told"
        );
    }

    /// An attempt whose clock was read before another call took the last
    /// token — a call reading the clock later, on another worker, took it
    /// first — is told the time from its own read to the next token, so the
    /// wait it computes ends there: with no wait it is refused for all of it,
    /// and with a wait of the window's length, which that token lies past, it
    /// is refused at once rather than woken before the token is back.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_read_before_another_took_the_last_token_is_told_from_its_own_read() {
        let window = Duration::from_secs(10);
        let later = Duration::from_millis(5);
        for wait in [None, Some(window)] {
            let policy = policy(rated(1, window, wait));
            let started = tokio::time::Instant::now();
            let rate = policy.mechanisms.rate.as_ref().expect("a rate limit");
            assert!(
                matches!(rate.arrive(started + later, None), Arrived::Taken),
                "the other call takes the token"
            );
            let Err(refused) = policy.run(answering).await else {
                panic!("with {wait:?} to wait, no token is free");
            };
            assert!(
                matches!(refused, ResilienceError::Refused { refusal: ResilienceRefusal::Limited { retry_after, .. }, .. }
                    if retry_after == window + later),
                "with {wait:?} to wait, told the token {later:?} past the window: {refused:?}"
            );
            assert_eq!(
                started.elapsed(),
                Duration::ZERO,
                "with {wait:?}, refused at once"
            );
            let problem = nest_rs_core::__private::find_in_chain::<Problem>(&refused)
                .expect("the problem rides the chain");
            assert_eq!(problem.retry_after(), Some(11), "rounded up");
        }
    }

    /// The call beside it reads the rate first, and would wait for the token
    /// after the one reserved, 19 s away: it is the bulkhead that refuses it.
    #[tokio::test(start_paused = true)]
    async fn the_bulkhead_slot_stays_held_while_an_attempt_waits_for_its_token() {
        let policy = policy(ResilienceConfig {
            concurrency: Some(1),
            ..rated(1, Duration::from_secs(10), Some(Duration::from_secs(20)))
        });
        assert!(policy.run(answering).await.is_ok());
        let (waited, refused) = tokio::join!(policy.run(answering), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            policy.run(answering).await
        });
        assert!(matches!(waited, Ok(())), "{waited:?}");
        assert!(
            matches!(
                refused,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Full,
                    ..
                })
            ),
            "the call waiting for its token holds the one slot: {refused:?}"
        );
    }

    /// A call finding no free slot reaches the rate once a slot frees, so its
    /// wait for its token is counted from there: one token every 10 s, the
    /// one slot held by a call waiting for the token at 10 s, a call arriving
    /// at 1 s is due the token at 20 s, 19 s away and past the rate's 15 s
    /// wait from its arrival. It waits for the slot, freed at 10 s, and for
    /// its token 10 s from there, and is sent.
    #[tokio::test(start_paused = true)]
    async fn a_call_finding_no_free_slot_is_sent_at_a_token_it_reaches_once_a_slot_frees() {
        let policy = policy(ResilienceConfig {
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(60)),
            ..rated(1, Duration::from_secs(10), Some(Duration::from_secs(15)))
        });
        assert!(policy.run(answering).await.is_ok());
        let started = tokio::time::Instant::now();
        let (waited, behind) = tokio::join!(policy.run(answering), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let answered = policy.run(answering).await;
            (answered, started.elapsed())
        });
        assert!(waited.is_ok(), "{waited:?}");
        assert!(
            matches!(behind, (Ok(()), at) if at == Duration::from_secs(20)),
            "the token at 20 s, 10 s from the slot freed at 10 s: {behind:?}"
        );
    }

    /// A call finding no free slot reads the rate before it waits for one, as
    /// `begin` reads the breaker: a bucket empty whose next token no wait of
    /// the call's reaches — past the longest it waits for a slot and the
    /// rate's wait from there, or past the unit's deadline — refuses it at
    /// once, told the time to that token, rather than once its wait for a slot
    /// ends. The read takes nothing: a call waiting for its slot beside a
    /// bucket with a token free is sent once the slot frees, taking that
    /// token.
    #[tokio::test(start_paused = true)]
    async fn a_call_finding_no_free_slot_is_refused_by_an_empty_bucket_before_it_waits() {
        let lines = nest_rs_testing::LogCapture::install();
        const TRACKING: ResilientMethod =
            crate::resilient::resilient_method(|| "ShipmentsService", "track", None, None);
        for (slot_wait, rate_wait, left, past) in [
            (
                Duration::from_secs(10),
                Duration::from_secs(5),
                None,
                "its 10 s wait for a slot and the rate's 5 s wait",
            ),
            (
                Duration::from_secs(60),
                Duration::from_secs(60),
                Some(Duration::from_secs(20)),
                "the unit's deadline 20 s away",
            ),
        ] {
            let policy = policy(ResilienceConfig {
                concurrency: Some(1),
                concurrency_wait: Some(slot_wait),
                ..rated(2, Duration::from_secs(60), Some(rate_wait))
            });
            let started = tokio::time::Instant::now();
            let (holding, waited) = tokio::join!(
                policy.run(|| async {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    Ok::<_, Refused>(())
                }),
                async {
                    tokio::task::yield_now().await;
                    let waited = policy.run(answering).await;
                    (started.elapsed(), waited)
                },
            );
            assert!(
                holding.is_ok(),
                "the bucket's first token, its call holding the slot 3 s: {holding:?}"
            );
            assert!(
                matches!(waited, (at, Ok(())) if at == Duration::from_secs(3)),
                "the bucket's second token, taken once the slot freed: {waited:?}"
            );

            let held = begin(&policy).await.expect("the slot, free again");
            let refused_at = tokio::time::Instant::now();
            let refused = nest_rs_core::__private::with_deadline(
                left.map(|left| refused_at + left),
                policy.run_once::<Ran, _, Cancellable, _>(
                    Some(&TRACKING),
                    judging_by_default(),
                    |_| answering(),
                ),
            )
            .await;
            assert!(
                matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Limited { retry_after, .. }, .. })
                    if retry_after == Duration::from_secs(27)),
                "the next token 27 s away, past {past}: {refused:?}"
            );
            assert_eq!(
                refused_at.elapsed(),
                Duration::ZERO,
                "refused at once, rather than after its wait for the slot held"
            );
            drop(held);
        }

        let limited = lines.find(TARGET, REFUSED_LIMITED);
        assert_eq!(limited.len(), 2, "{limited:#?}");
        for line in &limited {
            assert_eq!(line.level, "debug", "{line:?}");
            assert_eq!(line.field("instance").as_deref(), Some("upstream"));
            assert_eq!(line.field("host").as_deref(), Some("ShipmentsService"));
            assert_eq!(line.field("method").as_deref(), Some("track"));
            assert_eq!(line.field("retry_after_ms").as_deref(), Some("27000"));
        }
        assert!(
            lines.find(TARGET, REFUSED_FULL).is_empty(),
            "no call met the full bulkhead"
        );
    }

    /// The read counts the attempts waiting for a token, the call behind them
    /// all: one token every 10 s, the one slot held by a call waiting for the
    /// token at 10 s, a call arriving at 7 s may wait 3 s for a slot and 5 s
    /// for a token. The token at 10 s is the waiting call's, and its own, at
    /// 20 s, no wait reaches: it is refused at once, told that one, where a
    /// read of the next token alone would let it wait for the slot.
    #[tokio::test(start_paused = true)]
    async fn a_call_finding_no_free_slot_is_read_behind_the_attempts_waiting_for_a_token() {
        let policy = policy(ResilienceConfig {
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(3)),
            ..rated(1, Duration::from_secs(10), Some(Duration::from_secs(5)))
        });
        assert!(policy.run(answering).await.is_ok());
        let started = tokio::time::Instant::now();
        tokio::time::sleep(Duration::from_secs(6)).await;
        let (waited, behind) = tokio::join!(policy.run(answering), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let refused = policy.run(answering).await;
            (refused, started.elapsed())
        });
        assert!(waited.is_ok(), "{waited:?}");
        assert!(
            matches!(
                behind,
                (Err(ResilienceError::Refused { refusal: ResilienceRefusal::Limited { retry_after, .. }, .. }), at)
                    if retry_after == Duration::from_secs(13) && at == Duration::from_secs(7)
            ),
            "at once, told the token behind the one waited for: {behind:?}"
        );
    }

    /// The read refuses only a call no wait reaches a token for, and counts
    /// nothing: a call it lets wait for a slot meets the rate as it stands
    /// once it holds one, and is refused there by a token still past the
    /// rate's wait. One token every 10 s and no wait for one: the call may
    /// wait 60 s for a slot, the token back at 10 s within that, and the slot
    /// frees at 3 s, 7 s before the token.
    #[tokio::test(start_paused = true)]
    async fn a_call_let_wait_for_a_slot_is_refused_by_a_token_still_past_the_rates_wait() {
        let policy = policy(ResilienceConfig {
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(60)),
            ..rated(1, Duration::from_secs(10), None)
        });
        let started = tokio::time::Instant::now();
        let (holding, waited) = tokio::join!(
            policy.run(|| async {
                tokio::time::sleep(Duration::from_secs(3)).await;
                Ok::<_, Refused>(())
            }),
            async {
                tokio::task::yield_now().await;
                let waited = policy.run(answering).await;
                (waited, started.elapsed())
            },
        );
        assert!(holding.is_ok(), "{holding:?}");
        assert!(
            matches!(
                waited,
                (Err(ResilienceError::Refused { refusal: ResilienceRefusal::Limited { retry_after, .. }, .. }), at)
                    if retry_after == Duration::from_secs(7) && at == Duration::from_secs(3)
            ),
            "after its 3 s wait for the slot: {waited:?}"
        );
    }

    /// The rate a call finding no free slot reads comes after the unit's
    /// deadline and the breaker, in an attempt's order: beside the slot held
    /// and the bucket empty, a unit out of time is told so, and a call meeting
    /// an open breaker is told what is left of its opening, neither filing the
    /// rate's line; once neither refuses, the rate does, at once.
    #[tokio::test(start_paused = true)]
    async fn a_call_finding_no_free_slot_meets_the_units_deadline_and_the_breaker_before_the_rate()
    {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(ResilienceConfig {
            retries: None,
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(10)),
            rate_limit: Some(1),
            ..ResilienceConfig::default()
        });
        assert!(
            policy.run(answering).await.is_ok(),
            "the bucket's one token"
        );
        let held = begin(&policy).await.expect("the one slot");
        let started = tokio::time::Instant::now();

        let spent =
            nest_rs_core::__private::with_deadline(Some(started), policy.run(answering)).await;
        assert!(
            matches!(spent, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout.is_zero()),
            "out of time, before the spent rate: {spent:?}"
        );
        policy
            .open_breaker(Duration::from_secs(30))
            .expect("an open time `breaker_open` takes");
        let open = policy.run(answering).await;
        assert!(
            matches!(open, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Open { retry_after, .. }, .. })
                if retry_after == Duration::from_secs(30)),
            "the breaker opened by hand, before the spent rate: {open:?}"
        );
        assert!(
            lines.find(TARGET, REFUSED_LIMITED).is_empty(),
            "the rate refused neither"
        );

        policy.close_breaker().expect("a breaker");
        let limited = policy.run(answering).await;
        assert!(
            matches!(limited, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Limited { retry_after, .. }, .. })
                if retry_after == Duration::from_secs(60)),
            "the spent rate, once neither refuses: {limited:?}"
        );
        assert_eq!(
            started.elapsed(),
            Duration::ZERO,
            "each refused at once, never after a wait for the slot held"
        );
        drop(held);
    }

    /// A call finding no free slot while another holds the probe's turn — the
    /// probe's call out, or waiting for the rate's token with its slot held —
    /// is refused at once, told what is left of that probe's bound, rather than
    /// after a wait for the slot the probe holds.
    #[tokio::test(start_paused = true)]
    async fn a_call_finding_no_free_slot_beside_the_probe_is_refused_at_once_told_its_bound() {
        for (rate_limit, told, probe) in [
            (None, 4, "its call out, 1 s into its 5 s timeout"),
            (
                Some(1),
                13,
                "waiting for its token 8 s away, then for its 5 s timeout",
            ),
        ] {
            let policy = policy(ResilienceConfig {
                concurrency: Some(1),
                concurrency_wait: Some(Duration::from_secs(60)),
                rate_limit,
                rate_window: Duration::from_secs(10),
                rate_wait: Some(Duration::from_secs(10)),
                ..probing(Duration::from_secs(5))
            });
            probe_due(&policy).await;
            let (probed, refused) = tokio::join!(
                policy.run(|| async {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Ok::<_, Refused>(())
                }),
                async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let arrived = tokio::time::Instant::now();
                    let refused = policy.run(answering).await;
                    (arrived.elapsed(), refused)
                },
            );
            assert!(matches!(probed, Ok(())), "the probe {probe}: {probed:?}");
            assert!(
                matches!(refused, (waited, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Open { retry_after, .. }, .. }))
                    if waited.is_zero() && retry_after == Duration::from_secs(told)),
                "beside the probe {probe}, refused at once: {refused:?}"
            );
        }
    }

    /// The attempt's timeout starts when its call is sent, after the wait for
    /// its token; the unit's deadline bounds the wait and the attempt together.
    #[tokio::test(start_paused = true)]
    async fn the_wait_for_a_token_is_none_of_the_attempts_and_the_units_deadline_bounds_both() {
        let policy = policy(ResilienceConfig {
            timeout: Duration::from_secs(1),
            ..rated(1, Duration::from_secs(5), Some(Duration::from_secs(10)))
        });
        let slow = || async {
            tokio::time::sleep(Duration::from_millis(800)).await;
            Ok::<_, Refused>("answered")
        };
        assert!(policy.run(answering).await.is_ok());
        let started = tokio::time::Instant::now();
        let answered = policy.run(&slow).await;
        assert!(
            matches!(answered, Ok("answered")),
            "a 5 s wait, then 800 ms of its 1 s timeout: {answered:?}"
        );
        assert_eq!(started.elapsed(), Duration::from_millis(5_800));

        let started = tokio::time::Instant::now();
        let cut = nest_rs_core::__private::with_deadline(
            Some(started + Duration::from_millis(4_700)),
            policy.run(slow),
        )
        .await;
        assert!(
            matches!(cut, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. })
                if timeout == Duration::from_millis(500)),
            "a 4.2 s wait leaves 500 ms of the deadline, below the timeout: {cut:?}"
        );
        assert_eq!(started.elapsed(), Duration::from_millis(4_700));
    }

    /// A token is waited for only when it comes before the unit's deadline,
    /// but its timer may fire past it: the attempt is then refused as a unit
    /// out of time, unsent, and gives its token back.
    /// Its call never answers, so what was sent alone tells that refusal from
    /// a call sent with no time left.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_whose_wait_for_its_token_ends_past_the_units_deadline_is_refused_unsent() {
        let policy = policy(rated(
            1,
            Duration::from_secs(10),
            Some(Duration::from_secs(10)),
        ));
        assert!(policy.run(answering).await.is_ok());
        let sent = AtomicU32::new(0);
        let mut waiting = Box::pin(nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now() + Duration::from_secs(11)),
            policy.run(|| async {
                sent.fetch_add(1, Ordering::Relaxed);
                std::future::pending::<Result<(), Refused>>().await
            }),
        ));
        park(&mut waiting).await;
        tokio::time::advance(Duration::from_secs(12)).await;
        let late = waiting.await;
        assert!(
            matches!(late, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout.is_zero()),
            "its wait for the token at 10 s ended at 12 s, past its deadline at 11 s: {late:?}"
        );
        assert_eq!(
            sent.load(Ordering::Relaxed),
            0,
            "a unit out of time sends nothing"
        );
        let resumed = tokio::time::Instant::now();
        assert!(policy.run(answering).await.is_ok());
        assert_eq!(
            resumed.elapsed(),
            Duration::ZERO,
            "the next call takes that token at once"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_run_the_rate_refuses_ends_and_is_never_attempted_again() {
        let policy = policy(ResilienceConfig {
            retries: Some(3),
            ..rated(1, Duration::from_secs(60), None)
        });
        let calls = AtomicU32::new(0);
        let started = tokio::time::Instant::now();
        let answered = policy
            .run_idempotent(|| async {
                calls.fetch_add(1, Ordering::Relaxed);
                Err::<(), _>(Refused)
            })
            .await;
        assert!(
            matches!(
                answered,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Limited { .. },
                    ..
                })
            ),
            "its first retry was refused: {answered:?}"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        let waited = started.elapsed();
        assert!(
            waited <= Duration::from_millis(240),
            "the one backoff before the retry the rate refused, and none after it: {waited:?}"
        );

        let mut run = begin(&policy).await.expect("a run");
        let limited = Ended::Refused(ResilienceRefusal::Limited {
            retry_after: Duration::from_secs(1),
        });
        assert!(!again(run.as_mut(), limited).await);
    }

    /// A call dropped while it waits for its token leaves nothing behind: the
    /// next call takes its slot, and probes in its turn — a probe, since its
    /// failure opens the breaker again where a closed one would hold two.
    #[tokio::test(start_paused = true)]
    async fn a_caller_dropped_while_it_waits_frees_its_slot_and_hands_its_probe_on() {
        let policy = policy(ResilienceConfig {
            concurrency: Some(1),
            breaker_window: Some(2),
            breaker_failures: 2,
            breaker_open: Duration::from_secs(30),
            ..rated(2, Duration::from_secs(120), Some(Duration::from_secs(60)))
        });
        let calls = AtomicU32::new(0);
        let failing = || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Err::<(), _>(Refused)
        };
        for _ in 0..2 {
            let failed = policy.run(&failing).await;
            assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        }
        tokio::time::advance(Duration::from_secs(30)).await;

        let mut waiting = Box::pin(policy.run(&failing));
        park(&mut waiting).await;
        drop(waiting);

        let started = tokio::time::Instant::now();
        let probed = policy.run(failing).await;
        assert!(
            matches!(probed, Err(ResilienceError::Failed(Refused))),
            "the slot was freed and the breaker let it through: {probed:?}"
        );
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(30),
            "it waited for its token"
        );
        assert_eq!(
            calls.load(Ordering::Relaxed),
            3,
            "the dropped call was never sent"
        );
        let reopened = policy.run(answering).await;
        assert!(
            matches!(
                reopened,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                })
            ),
            "the next call was the probe: {reopened:?}"
        );
    }

    /// Runs its closure when it is dropped, beside the call holding it.
    struct OnDrop<'a>(&'a dyn Fn());

    impl Drop for OnDrop<'_> {
        fn drop(&mut self) {
            (self.0)();
        }
    }

    /// A run dropped while its probe's call is out hands the probe's turn on
    /// before its slot of the bulkhead: the call waiting for that slot, polled
    /// as soon as the freed slot wakes it — as a task another worker runs may
    /// be, before the drop goes on — takes the turn the dropped probe handed on.
    #[tokio::test(start_paused = true)]
    async fn a_run_dropped_while_its_probe_is_out_hands_the_turn_on_before_its_slot() {
        let policy: &'static ResiliencePolicy<Upstream> =
            Box::leak(Box::new(policy(ResilienceConfig {
                concurrency: Some(1),
                concurrency_wait: Some(Duration::from_secs(10)),
                retries: Some(1),
                backoff_first: Duration::from_secs(2),
                backoff_ceiling: Duration::from_secs(2),
                breaker_window: Some(1),
                breaker_failures: 1,
                breaker_open: Duration::from_secs(1),
                ..ResilienceConfig::default()
            })));
        let calls = AtomicU32::new(0);
        let mut holding = Box::pin(policy.run_idempotent(|| async {
            if calls.fetch_add(1, Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                return Err(Refused);
            }
            std::future::pending::<Result<(), Refused>>().await
        }));
        park(&mut holding).await;
        let waiting = PollsOnWake::new(policy.run(|| async { Ok::<_, Refused>("sent") }));
        waiting.poll_now();
        assert!(
            waiting.answered().is_none(),
            "it waits for the slot the run holds"
        );
        tokio::time::timeout(Duration::from_secs(5), holding.as_mut())
            .await
            .expect_err("the retry, sent as the probe, never answers");
        assert_eq!(calls.load(Ordering::Relaxed), 2, "the call and its retry");
        assert_eq!(policy.breaker_state(), Some(BreakerState::HalfOpen));

        drop(holding);
        let answer = waiting.answered();
        assert!(
            matches!(answer, Some(Ok("sent"))),
            "woken by the slot the dropped run freed, it took the turn its probe handed on: \
             {answer:?}"
        );
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Closed),
            "and closed the breaker as the next probe"
        );
    }

    /// A run sent once, dropped while its probe's call is out, hands the
    /// probe's turn on before its slot of the bulkhead as a run sent again
    /// does: the call that slot wakes takes the turn the dropped probe handed
    /// on.
    #[tokio::test(start_paused = true)]
    async fn a_run_sent_once_dropped_while_its_probe_is_out_hands_the_turn_on_before_its_slot() {
        let policy: &'static ResiliencePolicy<Upstream> =
            Box::leak(Box::new(policy(ResilienceConfig {
                concurrency: Some(1),
                concurrency_wait: Some(Duration::from_secs(10)),
                retries: None,
                breaker_window: Some(1),
                breaker_failures: 1,
                breaker_open: Duration::from_secs(1),
                ..ResilienceConfig::default()
            })));
        let mut holding = Box::pin(policy.run(|| async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok::<_, Refused>("held")
        }));
        park(&mut holding).await;
        let mut probing = Box::pin(policy.run(std::future::pending::<Result<&str, Refused>>));
        park(&mut probing).await;
        let waiting = PollsOnWake::new(policy.run(|| async { Ok::<_, Refused>("sent") }));
        waiting.poll_now();
        assert!(
            waiting.answered().is_none(),
            "it waits for the slot, behind the run that probes once it has it"
        );
        policy
            .open_breaker(Duration::from_secs(1))
            .expect("an open time `breaker_open` takes");
        assert!(matches!(holding.await, Ok("held")));
        park(&mut probing).await;
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::HalfOpen),
            "handed the slot, it probes, and its call never answers"
        );

        drop(probing);
        let answer = waiting.answered();
        assert!(
            matches!(answer, Some(Ok("sent"))),
            "woken by the slot the dropped run freed, it took the turn its probe handed on: \
             {answer:?}"
        );
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Closed),
            "and closed the breaker as the next probe"
        );
    }

    /// A run dropped while its probe waits for the rate's token hands the
    /// probe's turn on before its slot of the bulkhead too, run once or again:
    /// the call that slot wakes takes the turn, waits for that token in the
    /// dropped probe's place, and closes the breaker once it is sent.
    #[tokio::test(start_paused = true)]
    async fn a_run_dropped_while_its_probe_waits_for_its_token_hands_the_turn_on_before_its_slot() {
        for again in [false, true] {
            let policy: &'static ResiliencePolicy<Upstream> =
                Box::leak(Box::new(policy(ResilienceConfig {
                    concurrency: Some(1),
                    concurrency_wait: Some(Duration::from_secs(10)),
                    breaker_window: Some(1),
                    breaker_failures: 1,
                    breaker_open: Duration::from_secs(1),
                    ..rated(1, Duration::from_secs(60), Some(Duration::from_secs(60)))
                })));
            let mut failing = Box::pin(policy.run(|| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Err::<(), _>(Refused)
            }));
            park(&mut failing).await;
            let mut probing: Dropped<'_> = if again {
                Box::pin(policy.run_idempotent(answering))
            } else {
                Box::pin(policy.run(answering))
            };
            park(&mut probing).await;
            let waiting = PollsOnWake::new(policy.run(|| async { Ok::<_, Refused>("sent") }));
            waiting.poll_now();
            assert!(
                matches!(failing.await, Err(ResilienceError::Failed(Refused))),
                "the call that took the one token fails, opening the breaker for a second"
            );
            tokio::time::advance(Duration::from_secs(1)).await;
            park(&mut probing).await;
            assert!(
                waiting.answered().is_none(),
                "again: {again}; it waits for the slot the probe holds while it waits for its token"
            );

            drop(probing);
            let answer = waiting.answered();
            assert!(
                answer.is_none(),
                "again: {again}; woken by the slot the dropped run freed, it took the turn its \
                 probe handed on and waits for the token it gave back: {answer:?}"
            );
            tokio::time::sleep(Duration::from_secs(60)).await;
            let answer = waiting.answered();
            assert!(
                matches!(answer, Some(Ok("sent"))),
                "again: {again}; sent as the probe once that token is back: {answer:?}"
            );
            assert_eq!(
                policy.breaker_state(),
                Some(BreakerState::Closed),
                "again: {again}; and closed the breaker"
            );
        }
    }

    /// `policy`'s breaker opened by one failure, then its open second passed,
    /// so the next run probes.
    async fn opened_then_passed(policy: &ResiliencePolicy<Upstream>) {
        let failed = policy.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(1)).await;
    }

    /// A breaker one failure opens for a second, the next run its probe.
    fn probing_after_one_failure() -> ResiliencePolicy<Upstream> {
        policy(ResilienceConfig {
            retries: Some(1),
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(1),
            ..ResilienceConfig::default()
        })
    }

    /// An attempt holding its call counts a failure as its answer comes, and
    /// leaves a success still to settle: a probe keeps its turn until what the
    /// attempt left outstanding is dropped, the answer standing as judged, or
    /// is held past the run and settled there.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_holding_its_call_leaves_a_success_to_settle_and_counts_a_failure_at_once() {
        let policy = Arc::new(probing_after_one_failure());
        let answering = || async { Ok::<_, Refused>("answered") };

        opened_then_passed(&policy).await;
        let mut run = begin(&policy).await.expect("the probe's run");
        let attempted = attempt_once(run.as_mut(), answering, |_| failing(None)).await;
        assert!(matches!(
            attempted,
            (Ended::Judged(Verdict::Failed { .. }), Some(Ok("answered")))
        ));
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "a failure counts as it is judged, whatever is outstanding"
        );
        drop(run);

        tokio::time::advance(Duration::from_secs(1)).await;
        let mut run = begin(&policy).await.expect("the probe's run");
        let attempted = attempt_once(run.as_mut(), answering, judged).await;
        assert!(matches!(
            attempted,
            (Ended::Judged(Verdict::Succeeded), Some(Ok("answered")))
        ));
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::HalfOpen),
            "the probe's success is not counted yet"
        );
        drop(run);
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Closed),
            "dropped with the run, the success stands as judged"
        );

        opened_then_passed(&policy).await;
        let mut run = begin(&policy).await.expect("the probe's run");
        attempt_once(run.as_mut(), answering, judged).await;
        let mut held = run.as_mut().hold();
        assert_eq!(policy.breaker_state(), Some(BreakerState::HalfOpen));
        held.settle(Some("the answer's body was cut short"));
        assert!(matches!(
            policy.breaker_state(),
            Some(BreakerState::Open { .. })
        ));
        held.settle(None);
        drop(held);
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "settled once: a second verdict and the drop change nothing"
        );
    }

    /// What a run's answer holds past the run settles in a run's order, the
    /// probe's turn before the slot of the bulkhead: the call waiting for that
    /// slot, polled as soon as the freed slot wakes it, takes the turn the
    /// probe already handed on.
    #[tokio::test(start_paused = true)]
    async fn an_answer_held_past_its_run_hands_its_probes_turn_on_before_its_slot() {
        let policy = Arc::new(policy(ResilienceConfig {
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(10)),
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(1),
            ..ResilienceConfig::default()
        }));
        let mut run = begin(&policy).await.expect("the one slot");
        let next = Arc::clone(&policy);
        let waiting =
            PollsOnWake::new(async move { next.run(|| async { Ok::<_, Refused>("sent") }).await });
        waiting.poll_now();
        assert!(
            waiting.answered().is_none(),
            "it waits for the slot the run holds"
        );
        let failed = attempt_once(run.as_mut(), || async { Err::<(), _>(Refused) }, judged).await;
        assert!(matches!(
            failed,
            (Ended::Judged(Verdict::Final), Some(Err(Refused)))
        ));
        tokio::time::advance(Duration::from_secs(1)).await;
        attempt_once(run.as_mut(), || async { Ok::<_, Refused>(()) }, judged).await;
        let held = run.as_mut().hold();
        assert_eq!(policy.breaker_state(), Some(BreakerState::HalfOpen));

        drop(held);
        let answer = waiting.answered();
        assert!(
            matches!(answer, Some(Ok("sent"))),
            "woken by the slot the answer freed, it met the breaker its probe had closed: \
             {answer:?}"
        );
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
    }

    /// What an attempt holding its call answers of its own is what any attempt
    /// answers, counted the same: a call past its bound is given up there,
    /// timed out, and a call that panics counts as a failure and unwinds on.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_holding_its_call_is_given_up_at_its_bound_and_unwinds_as_any_other() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(ResilienceConfig {
            timeout: Duration::from_secs(2),
            breaker_window: Some(2),
            breaker_failures: 2,
            ..ResilienceConfig::default()
        });
        let sent = Instant::now();
        let mut run = begin(&policy).await.expect("a run");
        let (given_up, _) = attempt_once(
            run.as_mut(),
            std::future::pending::<Result<(), Refused>>,
            judged,
        )
        .await;
        assert!(
            matches!(given_up, Ended::Refused(ResilienceRefusal::TimedOut { timeout: bound }) if bound == Duration::from_secs(2)),
            "{given_up:?}"
        );
        assert_eq!(sent.elapsed(), Duration::from_secs(2));
        drop(run);
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));

        let mut run = begin(&policy).await.expect("a run");
        let panicking = attempt_once(
            run.as_mut(),
            || async {
                if true {
                    panic!("the upstream's client panicked");
                }
                Ok::<_, Refused>(())
            },
            judged,
        );
        assert!(nest_rs_core::panic::contain(panicking).await.is_err());
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "the timeout, then the panic: the two failures that open it"
        );
        assert_eq!(
            error_of(&lines, crate::teller::MOVED).as_deref(),
            Some(PANICKED)
        );
    }

    /// An answer that comes in the very poll its bound is due is the answer,
    /// for an attempt holding its call as for a run's: the call is polled
    /// before its bound.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_holding_its_call_answers_what_comes_in_the_poll_its_bound_is_due() {
        let timeout = Duration::from_secs(2);
        let policy = policy(ResilienceConfig {
            timeout,
            breaker_window: Some(1),
            breaker_failures: 1,
            ..ResilienceConfig::default()
        });
        let at_the_bound = || async move {
            tokio::time::sleep(timeout).await;
            Ok::<_, Refused>("answered")
        };

        let sent = Instant::now();
        let mut run = begin(&policy).await.expect("a run");
        let attempted = attempt_once(run.as_mut(), at_the_bound, judged).await;
        assert_eq!(sent.elapsed(), timeout);
        assert!(
            matches!(
                attempted,
                (Ended::Judged(Verdict::Succeeded), Some(Ok("answered")))
            ),
            "{attempted:?}"
        );
        drop(run);
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Closed),
            "no timeout was counted"
        );

        let ran = policy.run(at_the_bound).await;
        assert!(matches!(ran, Ok("answered")), "as a run's: {ran:?}");
    }

    /// An attempt holding its call arms its bound's timer at the call's first
    /// wait, as a run's does: one whose call answers at its first poll needs
    /// no clock.
    #[test]
    fn an_attempt_holding_its_call_arms_no_timer_for_a_call_answering_at_its_first_poll() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime with no time driver");
        let policy = policy(ResilienceConfig::default());
        let attempted = runtime.block_on(async {
            let mut run = begin(&policy).await.expect("a run");
            let answering = || async { Ok::<_, Refused>("answered") };
            attempt_once(run.as_mut(), answering, judged).await
        });
        assert!(
            matches!(
                attempted,
                (Ended::Judged(Verdict::Succeeded), Some(Ok("answered")))
            ),
            "{attempted:?}"
        );
    }

    /// What a run a test drops mid-call answers, run once or again alike.
    type Dropped<'a> = Pin<Box<dyn Future<Output = Result<(), ResilienceError<Refused>>> + 'a>>;

    /// A run dropped while its probe's call is out drops that call's future
    /// first, its attempt still holding the probe's turn: a call polled from the
    /// future's teardown is refused beside the probe, and the next takes the turn
    /// once the run is gone.
    #[tokio::test(start_paused = true)]
    async fn a_run_dropped_while_its_probe_is_out_drops_its_calls_future_before_the_turn() {
        for again in [false, true] {
            let policy = probing_after_one_failure();
            opened_then_passed(&policy).await;
            let beside = std::cell::RefCell::new(Box::pin(
                policy.run(|| async { Ok::<_, Refused>("beside") }),
            ));
            let answered = std::cell::RefCell::new(None);
            let poll_beside = || {
                let mut cx = Context::from_waker(std::task::Waker::noop());
                if let Poll::Ready(answer) = beside.borrow_mut().as_mut().poll(&mut cx) {
                    *answered.borrow_mut() = Some(answer);
                }
            };
            let probe = || async {
                let _torn_down = OnDrop(&poll_beside);
                std::future::pending::<Result<(), Refused>>().await
            };
            let mut probing: Dropped<'_> = if again {
                Box::pin(policy.run_idempotent(probe))
            } else {
                Box::pin(policy.run(probe))
            };
            let parked =
                std::future::poll_fn(|cx| Poll::Ready(probing.as_mut().poll(cx).is_pending()))
                    .await;
            assert!(parked, "the probe's call never answers");
            assert_eq!(policy.breaker_state(), Some(BreakerState::HalfOpen));

            drop(probing);
            let refused = answered.take();
            assert!(
                matches!(
                    refused,
                    Some(Err(ResilienceError::Refused {
                        refusal: ResilienceRefusal::Open { .. },
                        ..
                    }))
                ),
                "again: {again}; polled as the probe's future was torn down, it met the probe's \
                 turn still held: {refused:?}"
            );
            let next = policy.run(|| async { Ok::<_, Refused>("next") }).await;
            assert!(
                matches!(next, Ok("next")),
                "again: {again}; the turn was handed on with the run: {next:?}"
            );
        }
    }

    /// What the run a waker polls answers.
    type Woken = Result<&'static str, ResilienceError<Refused>>;

    /// A run polled at each wake, with a waker of its own, as a task another
    /// worker runs may be polled before the code that woke it goes on.
    struct PollsOnWake {
        run: std::sync::Mutex<Option<Pin<Box<dyn Future<Output = Woken> + Send>>>>,
        answered: std::sync::Mutex<Option<Woken>>,
    }

    impl PollsOnWake {
        /// `run`, first polled at a wake.
        fn new(run: impl Future<Output = Woken> + Send + 'static) -> Arc<Self> {
            Arc::new(Self {
                run: std::sync::Mutex::new(Some(Box::pin(run))),
                answered: std::sync::Mutex::new(None),
            })
        }

        /// The run polled now, as a wake polls it.
        fn poll_now(self: &Arc<Self>) {
            std::task::Wake::wake_by_ref(self);
        }

        /// What the run answered, once it has.
        fn answered(&self) -> Option<Woken> {
            self.answered.lock().expect("one wake at a time").take()
        }
    }

    impl std::task::Wake for PollsOnWake {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            let mut run = self.run.lock().expect("one wake at a time");
            let Some(polled) = run.as_mut() else {
                return;
            };
            let waker = std::task::Waker::from(Arc::clone(self));
            if let Poll::Ready(answer) = polled.as_mut().poll(&mut Context::from_waker(&waker)) {
                *run = None;
                *self.answered.lock().expect("one wake at a time") = Some(answer);
            }
        }
    }

    /// An attempt dropped while its probe's call is out hands the probe's turn
    /// on before it cancels its token: a task the cancellation wakes, polled at
    /// once, takes the turn the attempt no longer holds.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_dropped_while_its_probe_is_out_hands_the_turn_on_before_it_cancels_its_token()
     {
        let policy: &'static ResiliencePolicy<Upstream> =
            Box::leak(Box::new(probing_after_one_failure()));
        for again in [false, true] {
            opened_then_passed(policy).await;
            let handed = std::cell::RefCell::new(None);
            let probe = |cancel: CancellationToken| {
                *handed.borrow_mut() = Some(cancel);
                std::future::pending::<Result<(), Refused>>()
            };
            let mut probing: Dropped<'_> = if again {
                Box::pin(policy.run_idempotent_cancellable(probe))
            } else {
                Box::pin(policy.run_cancellable(probe))
            };
            let parked =
                std::future::poll_fn(|cx| Poll::Ready(probing.as_mut().poll(cx).is_pending()))
                    .await;
            assert!(parked, "the probe's call never answers");
            assert_eq!(policy.breaker_state(), Some(BreakerState::HalfOpen));
            let token = handed.take().expect("the probe's call was made");
            let woken = PollsOnWake::new(policy.run(|| async { Ok::<_, Refused>("woken") }));
            let waker = std::task::Waker::from(Arc::clone(&woken));
            let mut cancelled = Box::pin(token.cancelled_owned());
            assert!(
                cancelled
                    .as_mut()
                    .poll(&mut Context::from_waker(&waker))
                    .is_pending()
            );

            drop(probing);
            let answered = woken.answered();
            assert!(
                matches!(answered, Some(Ok("woken"))),
                "again: {again}; woken by the cancellation, it took the turn the dropped attempt \
                 handed on: {answered:?}"
            );
        }
    }

    /// The attempt's bound ends at the unit's deadline itself, however long its
    /// call took to first wait — a request built, a handshake begun: the edge
    /// cutting the unit at that deadline, as each edge does, finds the attempt
    /// given up already, and counted in the breaker.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_slow_to_first_wait_ends_at_the_deadline_its_edge_cuts_the_unit_at() {
        let policy = policy(ResilienceConfig {
            timeout: Duration::from_secs(5),
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            ..ResilienceConfig::default()
        });
        let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
        let cut = tokio::time::timeout_at(
            deadline,
            nest_rs_core::__private::with_deadline(
                Some(deadline),
                policy.run(|| async {
                    tokio::time::advance(Duration::from_millis(3)).await;
                    std::future::pending::<Result<(), Refused>>().await
                }),
            ),
        )
        .await;
        assert!(
            matches!(cut, Ok(Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }))
                if timeout == Duration::from_millis(300)),
            "the policy gave the attempt up at the deadline, before the edge cut it: {cut:?}"
        );
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "the attempt counted in the breaker: {:?}",
            policy.breaker_state()
        );
    }

    /// A probe slow to first wait whose unit's deadline comes before its
    /// timeout has failed once its edge cuts the unit there, as one cut at its
    /// timeout has: the breaker opens again for its whole open time.
    #[tokio::test(start_paused = true)]
    async fn a_probe_slow_to_first_wait_cut_at_its_units_deadline_has_failed() {
        let policy = policy(probing(Duration::from_secs(5)));
        probe_due(&policy).await;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
        let cut = tokio::time::timeout_at(
            deadline,
            nest_rs_core::__private::with_deadline(
                Some(deadline),
                policy.run(|| async {
                    tokio::time::advance(Duration::from_millis(3)).await;
                    std::future::pending::<Result<(), Refused>>().await
                }),
            ),
        )
        .await;
        assert!(
            matches!(
                cut,
                Ok(Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::TimedOut { .. },
                    ..
                }))
            ),
            "{cut:?}"
        );
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Open {
                retry_after: Duration::from_secs(1)
            }),
            "the probe failed, opening the breaker again for its whole open time"
        );
    }

    /// An idempotent run's attempt ends at its unit's deadline as a run sent
    /// once does, however long its call takes to first wait: given up there
    /// before the edge cuts the unit, counted in the breaker, and not sent
    /// again with no time left.
    #[tokio::test(start_paused = true)]
    async fn an_idempotent_attempt_slow_to_first_wait_ends_at_the_deadline_its_edge_cuts_the_unit_at()
     {
        let policy = policy(ResilienceConfig {
            timeout: Duration::from_secs(5),
            retries: Some(1),
            breaker_window: Some(1),
            breaker_failures: 1,
            ..ResilienceConfig::default()
        });
        let calls = AtomicU32::new(0);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
        let cut = tokio::time::timeout_at(
            deadline,
            nest_rs_core::__private::with_deadline(
                Some(deadline),
                policy.run_idempotent(|| async {
                    calls.fetch_add(1, Ordering::Relaxed);
                    tokio::time::advance(Duration::from_millis(3)).await;
                    std::future::pending::<Result<(), Refused>>().await
                }),
            ),
        )
        .await;
        assert!(
            matches!(cut, Ok(Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }))
                if timeout == Duration::from_millis(300)),
            "the policy gave the attempt up at the deadline, before the edge cut it: {cut:?}"
        );
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "not sent again, with no time left"
        );
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "the attempt counted in the breaker: {:?}",
            policy.breaker_state()
        );
    }

    /// `unit` under its `deadline`, cut there by an edge that sees its own
    /// timer before the unit's — an order a runtime of several workers may
    /// poll two timers due at one instant in, tokio firing a tick's timers in
    /// batches — answering `None` once it cut the unit.
    async fn cut_first_at<F: Future>(deadline: Instant, unit: F) -> Option<F::Output> {
        let unit = nest_rs_core::__private::with_deadline(Some(deadline), unit);
        tokio::select! {
            biased;
            () = tokio::time::sleep_until(deadline) => None,
            answered = unit => Some(answered),
        }
    }

    /// An attempt its edge cuts at the deadline it is bound by, before the
    /// policy saw its own timer due there fire, has timed out all the same:
    /// the breaker counts it, whichever of the two timers the runtime polls
    /// first, sent once or again. One its caller drops before its bound ends
    /// counts for nothing.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_its_edge_cuts_first_at_its_deadline_counts_as_timed_out_in_the_breaker() {
        /// A run of a call that never answers, sent once or again.
        fn silent_run(
            policy: &ResiliencePolicy<Upstream>,
            idempotent: bool,
        ) -> Pin<Box<dyn Future<Output = Result<(), ResilienceError<Refused>>> + '_>> {
            let call = || std::future::pending::<Result<(), Refused>>();
            if idempotent {
                Box::pin(policy.run_idempotent(call))
            } else {
                Box::pin(policy.run(call))
            }
        }

        for idempotent in [false, true] {
            let policy = policy(ResilienceConfig {
                timeout: Duration::from_secs(5),
                retries: Some(1),
                breaker_window: Some(2),
                breaker_failures: 1,
                ..ResilienceConfig::default()
            });
            let silent = || silent_run(&policy, idempotent);
            let deadline = Instant::now() + Duration::from_millis(300);
            let dropped = tokio::time::timeout(
                Duration::from_millis(100),
                nest_rs_core::__private::with_deadline(Some(deadline), silent()),
            )
            .await;
            assert!(dropped.is_err(), "its caller dropped it 200 ms early");
            assert_eq!(
                policy.breaker_state(),
                Some(BreakerState::Closed),
                "dropped before its bound ended, it counted for nothing"
            );

            let cut = cut_first_at(Instant::now() + Duration::from_millis(300), silent()).await;
            assert!(cut.is_none(), "the edge cut the unit first: {cut:?}");
            assert!(
                matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
                "sent {}, the attempt cut at its bound counted as timed out: {:?}",
                if idempotent { "again" } else { "once" },
                policy.breaker_state()
            );
        }
    }

    /// A probe its edge cuts at its unit's deadline, before the policy saw its
    /// own timer due there fire, has failed all the same: the breaker opens
    /// again for its whole open time, rather than handing the probe's turn to
    /// the next call at once.
    #[tokio::test(start_paused = true)]
    async fn a_probe_its_edge_cuts_first_at_its_units_deadline_has_failed() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(probing(Duration::from_secs(5)));
        probe_due(&policy).await;
        let cut = cut_first_at(
            Instant::now() + Duration::from_millis(300),
            policy.run(std::future::pending::<Result<(), Refused>>),
        )
        .await;
        assert!(cut.is_none(), "the edge cut the unit first: {cut:?}");
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Open {
                retry_after: Duration::from_secs(1)
            }),
            "the probe failed, opening the breaker again for its whole open time"
        );
        let reopened = lines
            .find(TARGET, crate::teller::MOVED)
            .pop()
            .expect("the probe's transition");
        assert_eq!(
            (reopened.field("state"), reopened.field("failed")),
            (Some("open".to_owned()), Some("1".to_owned())),
            "filed as a failed probe: {reopened:?}"
        );
    }

    /// A call refused while the probe waits for the rate's token is told when
    /// the probe has answered at the latest: what is left of that wait, then
    /// its call's timeout.
    #[tokio::test(start_paused = true)]
    async fn a_call_refused_while_the_probe_waits_for_its_token_is_told_that_wait_then_its_timeout()
    {
        let policy = policy(ResilienceConfig {
            timeout: Duration::from_secs(2),
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(30),
            ..rated(1, Duration::from_secs(60), Some(Duration::from_secs(40)))
        });
        let failed = policy.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(30)).await;
        let (probed, refused) = tokio::join!(policy.run(answering), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            policy.run(answering).await
        });
        assert!(matches!(probed, Ok(())), "{probed:?}");
        assert!(
            matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Open { retry_after, .. }, .. })
                if retry_after == Duration::from_secs(31)),
            "at 31 s, the probe waiting for its token at 60 s, then 2 s: {refused:?}"
        );
    }

    /// A breaker one failure opens for a second, each attempt given up past
    /// `timeout`, no retry: the subject is what a call refused while the probe
    /// is out is told.
    fn probing(timeout: Duration) -> ResilienceConfig {
        ResilienceConfig {
            timeout,
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(1),
            ..ResilienceConfig::default()
        }
    }

    /// Opens `policy`'s breaker by one failure, and lets its open time pass.
    async fn probe_due(policy: &ResiliencePolicy<Upstream>) {
        let failed = policy.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(1)).await;
    }

    /// Once the probe's call is sent no wait for a token is left in its
    /// bound: a call refused while it is out is told what is left of its
    /// timeout, whatever the rate's wait.
    #[tokio::test(start_paused = true)]
    async fn a_call_refused_while_the_probes_call_is_out_is_told_what_is_left_of_its_timeout() {
        let policy = policy(ResilienceConfig {
            rate_limit: Some(10),
            rate_window: Duration::from_secs(60),
            rate_wait: Some(Duration::from_secs(60)),
            ..probing(Duration::from_secs(5))
        });
        probe_due(&policy).await;
        let (probed, refused) = tokio::join!(
            policy.run(|| async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                Ok::<_, Refused>(())
            }),
            async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                policy.run(answering).await
            },
        );
        assert!(matches!(probed, Ok(())), "{probed:?}");
        assert!(
            matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Open { retry_after, .. }, .. })
                if retry_after == Duration::from_secs(4)),
            "the probe's call sent at once, 1 s into its 5 s timeout: {refused:?}"
        );
    }

    /// A probe from a method pinning its own timeout is waited for as long.
    #[tokio::test(start_paused = true)]
    async fn a_call_refused_while_a_method_pinning_its_timeout_probes_is_told_what_is_left_of_it() {
        let policy = policy(probing(Duration::from_secs(5)));
        probe_due(&policy).await;
        const TRACKING: ResilientMethod =
            crate::resilient::resilient_method(|| "ShipmentsService", "track", Some(30_000), None);
        let (probed, refused) = tokio::join!(
            policy.run_once::<Ran, _, Cancellable, _>(
                Some(&TRACKING),
                judging_by_default(),
                |_| async {
                    tokio::time::sleep(Duration::from_secs(20)).await;
                    Ok::<_, Refused>(())
                }
            ),
            async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                policy.run(answering).await
            },
        );
        assert!(
            matches!(probed, Ok(())),
            "20 s of the method's 30 s: {probed:?}"
        );
        assert!(
            matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Open { retry_after, .. }, .. })
                if retry_after == Duration::from_secs(29)),
            "1 s into the method's 30 s, past the policy's 5 s: {refused:?}"
        );
    }

    /// A probe whose unit ends before its timeout is given up at the unit's
    /// deadline, and waited for until then.
    #[tokio::test(start_paused = true)]
    async fn a_call_refused_while_a_probe_runs_out_its_units_deadline_is_told_what_is_left_of_it() {
        let policy = policy(probing(Duration::from_secs(5)));
        probe_due(&policy).await;
        let (probed, refused) = tokio::join!(
            nest_rs_core::__private::with_deadline(
                Some(tokio::time::Instant::now() + Duration::from_secs(2)),
                policy.run(std::future::pending::<Result<(), Refused>>),
            ),
            async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                policy.run(answering).await
            },
        );
        assert!(
            matches!(probed, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. })
                if timeout == Duration::from_secs(2)),
            "{probed:?}"
        );
        assert!(
            matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Open { retry_after, .. }, .. })
                if retry_after == Duration::from_secs(1)),
            "1 s into the 2 s its unit had left, under the 5 s timeout: {refused:?}"
        );
    }

    /// A probe holding its turn while it waits for the rate's token is waited
    /// for until that token, then for its own bound: the timeout its method
    /// pinned, or what its unit's deadline leaves of the policy's.
    #[tokio::test(start_paused = true)]
    async fn a_probe_waiting_for_its_token_is_waited_for_until_it_then_for_its_own_bound() {
        const TRACKING: ResilientMethod =
            crate::resilient::resilient_method(|| "ShipmentsService", "track", Some(30_000), None);
        for (method, left, told, bound) in [
            (
                Some(&TRACKING),
                None,
                38,
                "the method's 30 s, past the policy's 5 s",
            ),
            (
                None,
                Some(Duration::from_secs(11)),
                10,
                "the 2 s its unit's deadline at 12 s leaves of the policy's 5 s",
            ),
        ] {
            let policy = policy(ResilienceConfig {
                rate_limit: Some(1),
                rate_window: Duration::from_secs(10),
                rate_wait: Some(Duration::from_secs(10)),
                ..probing(Duration::from_secs(5))
            });
            probe_due(&policy).await;
            let deadline = left.map(|left| tokio::time::Instant::now() + left);
            let (probed, refused) = tokio::join!(
                nest_rs_core::__private::with_deadline(
                    deadline,
                    policy.run_once::<Ran, _, Cancellable, _>(method, judging_by_default(), |_| {
                        answering()
                    })
                ),
                async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    policy.run(answering).await
                },
            );
            assert!(matches!(probed, Ok(())), "sent at its token: {probed:?}");
            assert!(
                matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Open { retry_after, .. }, .. })
                    if retry_after == Duration::from_secs(told)),
                "at 2 s, the probe sent at its token's instant, 10 s, under {bound}: {refused:?}"
            );
        }
    }

    /// A probe whose call is cut at its timeout has failed, wherever it stands
    /// in its series: the breaker opens again for its whole open time, and the
    /// series starts over.
    #[tokio::test(start_paused = true)]
    async fn a_probe_cut_at_its_timeout_has_failed_and_the_series_starts_over() {
        let policy = policy(ResilienceConfig {
            breaker_probes: 2,
            ..probing(Duration::from_secs(5))
        });
        probe_due(&policy).await;
        assert!(policy.run(answering).await.is_ok(), "one probe of two");
        let cut = policy
            .run(std::future::pending::<Result<(), Refused>>)
            .await;
        assert!(
            matches!(cut, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. })
                if timeout == Duration::from_secs(5)),
            "{cut:?}"
        );
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Open {
                retry_after: Duration::from_secs(1)
            }),
            "opened again for its whole open time"
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(policy.run(answering).await.is_ok());
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::HalfOpen),
            "one probe of two since it opened again"
        );
        assert!(policy.run(answering).await.is_ok());
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
    }

    /// An attempt the closed breaker let through asks it again once its wait
    /// for its token ends: a breaker another call opened meanwhile refuses it
    /// unsent, as it refuses a call made at that instant.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_asks_the_breaker_again_after_its_wait_and_one_opened_meanwhile_refuses_it()
    {
        let policy = policy(ResilienceConfig {
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(30),
            ..rated(1, Duration::from_secs(10), Some(Duration::from_secs(10)))
        });
        let sent = AtomicU32::new(0);
        let started = tokio::time::Instant::now();
        let (failed, waited) = tokio::join!(
            policy.run(|| async {
                sent.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_secs(1)).await;
                Err::<(), _>(Refused)
            }),
            async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                policy
                    .run(|| async {
                        sent.fetch_add(1, Ordering::Relaxed);
                        Ok::<_, Refused>(())
                    })
                    .await
            },
        );
        assert!(
            matches!(failed, Err(ResilienceError::Failed(Refused))),
            "{failed:?}"
        );
        assert!(
            matches!(waited, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Open { retry_after, .. }, .. })
                if retry_after == Duration::from_secs(21)),
            "the failure at 1 s opened the breaker until 31 s: {waited:?}"
        );
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(10),
            "refused at the instant of the token it waited for"
        );
        assert_eq!(
            sent.load(Ordering::Relaxed),
            1,
            "the attempt refused there is never sent"
        );
    }

    /// An attempt waiting for its token while another call took the probe's
    /// turn is refused when its wait ends: one call probes the dependency at a
    /// time, and every other is told when it has answered at the latest. The
    /// token it gives back goes to the probe, in line behind it, sent there.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_waiting_for_its_token_is_never_sent_beside_the_probe() {
        let policy = policy(ResilienceConfig {
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(2),
            ..rated(2, Duration::from_secs(10), Some(Duration::from_secs(10)))
        });
        let sent = std::sync::Mutex::new(Vec::new());
        let call = |name: &'static str, answer: Result<(), Refused>, takes: Duration| {
            let sent = &sent;
            move || async move {
                sent.lock().expect("the calls sent").push(name);
                tokio::time::sleep(takes).await;
                answer
            }
        };
        let started = tokio::time::Instant::now();
        let (_, _, beside, probed) = tokio::join!(
            policy.run(call("opening", Err(Refused), Duration::from_secs(1))),
            policy.run(call("filling", Ok(()), Duration::ZERO)),
            async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                policy.run(call("beside", Ok(()), Duration::ZERO)).await
            },
            async {
                tokio::time::sleep(Duration::from_secs(4)).await;
                policy
                    .run(call("probe", Ok(()), Duration::from_secs(1)))
                    .await
            },
        );
        assert!(matches!(probed, Ok(())), "{probed:?}");
        assert!(
            matches!(beside, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Open { retry_after, .. }, .. })
                if retry_after == Duration::from_secs(10)),
            "at 5 s, the probe due the token after its own, at 10 s, then its 5 s timeout: \
             {beside:?}"
        );
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(6),
            "the probe sent at 5 s, on the token the refused attempt gave back, and answered \
             a second on"
        );
        assert_eq!(
            *sent.lock().expect("the calls sent"),
            ["opening", "filling", "probe"]
        );
    }

    /// An attempt whose breaker opened, and whose open time passed, while it
    /// waited for its token is the probe when its wait ends, so its answer
    /// closes the breaker.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_whose_breakers_open_time_passed_while_it_waited_is_sent_as_the_probe() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(ResilienceConfig {
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(2),
            ..rated(1, Duration::from_secs(10), Some(Duration::from_secs(10)))
        });
        let (failed, probed) = tokio::join!(
            policy.run(|| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Err::<(), _>(Refused)
            }),
            async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                policy.run(answering).await
            },
        );
        assert!(
            matches!(failed, Err(ResilienceError::Failed(Refused))),
            "{failed:?}"
        );
        assert!(matches!(probed, Ok(())), "{probed:?}");
        let states: Vec<String> = lines
            .find(TARGET, crate::teller::MOVED)
            .iter()
            .map(|line| line.field("state").unwrap_or_default())
            .collect();
        assert_eq!(
            states,
            ["open", "half_open", "closed"],
            "the attempt that waited probed, and its answer closed the breaker"
        );
    }

    /// A probe given up before its call is sent — refused by the rate, or its
    /// caller dropped while it waits for its token — files no transition: the
    /// breaker reads half-open from the probe's call sent.
    #[tokio::test(start_paused = true)]
    async fn a_probe_given_up_before_its_call_is_sent_files_no_transition() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(ResilienceConfig {
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(1),
            ..rated(1, Duration::from_secs(60), Some(Duration::from_secs(30)))
        });
        let states = || -> Vec<String> {
            lines
                .find(TARGET, crate::teller::MOVED)
                .iter()
                .map(|line| {
                    assert_eq!(line.level, "warn", "{line:?}");
                    line.field("state").unwrap_or_default()
                })
                .collect()
        };
        let failed = policy.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(1)).await;
        for _ in 0..10 {
            let refused = policy.run(answering).await;
            assert!(
                matches!(
                    refused,
                    Err(ResilienceError::Refused {
                        refusal: ResilienceRefusal::Limited { .. },
                        ..
                    })
                ),
                "a token 59 s away, past the 30 s wait: {refused:?}"
            );
        }
        assert_eq!(states(), ["open"], "ten probes the rate refused, none sent");

        tokio::time::advance(Duration::from_secs(34)).await;
        let mut waiting = Box::pin(policy.run(answering));
        park(&mut waiting).await;
        drop(waiting);
        assert_eq!(
            states(),
            ["open"],
            "a probe dropped while it waited, never sent"
        );

        tokio::time::advance(Duration::from_secs(25)).await;
        assert!(policy.run(answering).await.is_ok());
        assert_eq!(
            states(),
            ["open", "half_open", "closed"],
            "the probe sent once a token is back"
        );
    }

    /// A probe whose wait for its token ends past its unit's deadline is given
    /// up unsent too, as a unit out of time: it files no transition, gives its
    /// token back, and the next call probes in its turn.
    #[tokio::test(start_paused = true)]
    async fn a_probe_whose_wait_for_its_token_ends_past_the_units_deadline_hands_its_turn_on() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(ResilienceConfig {
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(1),
            ..rated(1, Duration::from_secs(10), Some(Duration::from_secs(10)))
        });
        let states = || -> Vec<String> {
            lines
                .find(TARGET, crate::teller::MOVED)
                .iter()
                .map(|line| line.field("state").unwrap_or_default())
                .collect()
        };
        let failed = policy.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(1)).await;
        let mut probing = Box::pin(nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now() + Duration::from_secs(10)),
            policy.run(answering),
        ));
        park(&mut probing).await;
        tokio::time::advance(Duration::from_secs(11)).await;
        let late = probing.await;
        assert!(
            matches!(late, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout.is_zero()),
            "its wait for the token at 10 s ended at 12 s, past its deadline at 11 s: {late:?}"
        );
        assert_eq!(states(), ["open"], "the probe given up, never sent");

        let resumed = tokio::time::Instant::now();
        assert!(policy.run(answering).await.is_ok());
        assert_eq!(
            resumed.elapsed(),
            Duration::ZERO,
            "the next call takes that token at once"
        );
        assert_eq!(
            states(),
            ["open", "half_open", "closed"],
            "the next call probed"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_refusal_by_the_rate_and_a_wait_for_a_token_are_debug_lines_naming_the_instance() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(rated(
            1,
            Duration::from_secs(10),
            Some(Duration::from_secs(5)),
        ));
        assert!(policy.run(answering).await.is_ok());
        tokio::time::advance(Duration::from_secs(4)).await;
        let refused = policy.run(answering).await;
        assert!(
            matches!(
                refused,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Limited { .. },
                    ..
                })
            ),
            "{refused:?}"
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(policy.run(answering).await.is_ok());

        let refusal = lines.expect_one(TARGET, REFUSED_LIMITED);
        let wait = lines.expect_one(TARGET, WAITING_FOR_TOKEN);
        for (line, field, said) in [
            (&refusal, "retry_after_ms", "6000"),
            (&wait, "wait_ms", "4000"),
        ] {
            assert_eq!(line.level, "debug", "{line:?}");
            assert_eq!(line.field("instance").as_deref(), Some("upstream"));
            assert_eq!(line.field(field).as_deref(), Some(said), "{line:?}");
            assert_eq!(
                (line.field("host"), line.field("method")),
                (None, None),
                "a call run through the policy is no method"
            );
        }
    }

    /// A token under a millisecond away is filed in whole milliseconds rounded
    /// up, `1`, never the `0` of no wait: told an attempt the rate refuses, and
    /// waited for by one whose wait reaches it, which is sent once the token
    /// is back, never before.
    #[tokio::test(start_paused = true)]
    async fn a_token_under_a_millisecond_away_is_filed_as_one_never_as_zero() {
        let lines = nest_rs_testing::LogCapture::install();
        let window = Duration::from_secs(1);
        let refusing = policy(rated(1, window, None));
        let waiting = policy(rated(1, window, Some(Duration::from_millis(1))));
        for policy in [&refusing, &waiting] {
            assert!(policy.run(answering).await.is_ok(), "the one token");
        }
        let near = Duration::from_micros(500);
        tokio::time::advance(window - near).await;
        let refused = refusing.run(answering).await;
        assert!(
            matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Limited { retry_after, .. }, .. }) if retry_after == near),
            "{refused:?}"
        );
        let back = tokio::time::Instant::now() + near;
        let sent = std::cell::Cell::new(None);
        let answered = waiting
            .run(|| async {
                sent.set(Some(tokio::time::Instant::now()));
                Ok::<_, Refused>(())
            })
            .await;
        assert!(answered.is_ok(), "{answered:?}");
        assert!(
            sent.get().is_some_and(|sent| sent >= back),
            "sent once the token is back: {:?}",
            sent.get()
        );

        let refusal = lines.expect_one(TARGET, REFUSED_LIMITED);
        let wait = lines.expect_one(TARGET, WAITING_FOR_TOKEN);
        for (line, field) in [(&refusal, "retry_after_ms"), (&wait, "wait_ms")] {
            assert_eq!(line.field(field).as_deref(), Some("1"), "{line:?}");
        }
    }

    /// What the upstream answered, down: a chain of two links.
    #[derive(Debug, thiserror::Error)]
    #[error("the upstream is unreachable")]
    struct Unreachable(#[source] std::io::Error);

    const UNREACHABLE: &str = "the upstream is unreachable: connection reset by peer";

    fn unreachable() -> Unreachable {
        Unreachable(std::io::Error::other("connection reset by peer"))
    }

    /// The `error` of the one line on this crate's target saying `message`.
    fn error_of(lines: &nest_rs_testing::LogCapture, message: &str) -> Option<String> {
        lines.expect_one(TARGET, message).field("error")
    }

    /// Each line a retry files says what its attempt failed on: the error's
    /// whole chain, or the timeout's sentence for an attempt given up at its
    /// bound — the retry made, and the two given up, the dependency asking a
    /// wait past the ceiling and the unit's deadline leaving no time for it.
    #[tokio::test(start_paused = true)]
    async fn each_line_a_retry_files_says_what_its_attempt_failed_on() {
        let policy = retrying(1);

        let lines = nest_rs_testing::LogCapture::install();
        let failed = policy
            .run_idempotent(|| async { Err::<(), _>(unreachable()) })
            .await;
        assert!(
            matches!(failed, Err(ResilienceError::Failed(_))),
            "{failed:?}"
        );
        assert_eq!(
            error_of(&lines, RETRYING_FAILED).as_deref(),
            Some(UNREACHABLE)
        );
        drop(lines);

        let lines = nest_rs_testing::LogCapture::install();
        let timed_out = policy
            .run_idempotent(std::future::pending::<Result<(), Refused>>)
            .await;
        assert!(
            matches!(
                timed_out,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::TimedOut { .. },
                    ..
                })
            ),
            "{timed_out:?}"
        );
        assert_eq!(
            error_of(&lines, RETRYING_FAILED).as_deref(),
            Some(TIMED_OUT)
        );
        drop(lines);

        let lines = nest_rs_testing::LogCapture::install();
        let asking =
            || Throttled(Problem::new(429, nest_rs_core::code::RATE_LIMITED).with_retry_after(60));
        let asked = policy
            .run_idempotent(|| async move { Err::<(), _>(asking()) })
            .await;
        assert!(
            matches!(asked, Err(ResilienceError::Failed(_))),
            "{asked:?}"
        );
        assert_eq!(
            error_of(&lines, NOT_ATTEMPTED_AGAIN_PAST_CEILING,),
            Some(error_message(&asking())),
        );
        drop(lines);

        let lines = nest_rs_testing::LogCapture::install();
        let out_of_time = nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now() + Duration::from_millis(1)),
            policy.run_idempotent(|| async { Err::<(), _>(unreachable()) }),
        )
        .await;
        assert!(
            matches!(out_of_time, Err(ResilienceError::Failed(_))),
            "{out_of_time:?}"
        );
        assert_eq!(
            error_of(&lines, NOT_ATTEMPTED_AGAIN_PAST_DEADLINE,).as_deref(),
            Some(UNREACHABLE),
        );
    }

    /// A retry's line says what its attempt did, one sentence a fact: it
    /// failed, for an `Err` sent again — one counting in the breaker as
    /// neither a failure nor a success included — and it answered, with no
    /// error to tell, for an `Ok` sent again, which nothing failed in.
    #[tokio::test(start_paused = true)]
    async fn the_line_a_retry_files_says_whether_its_attempt_failed_or_answered() {
        type Polled = Answer<u8, Refused>;
        let policy = retrying(1);

        let lines = nest_rs_testing::LogCapture::install();
        let ignored = policy
            .run_again::<Polled, _, Cancellable, _>(
                None,
                judging(PhantomData::<Polled>, every, none, none),
                |_| async { Err(Refused) },
            )
            .await;
        assert!(
            matches!(ignored, Err(ResilienceError::Failed(Refused))),
            "{ignored:?}"
        );
        assert_eq!(
            error_of(&lines, RETRYING_FAILED),
            Some(error_message(&Refused)),
            "an `Err` `record_if` refuses, sent again"
        );
        lines.expect_none(TARGET, RETRYING_ANSWERED);
        drop(lines);

        let lines = nest_rs_testing::LogCapture::install();
        let polled = policy
            .run_again::<Polled, _, Cancellable, _>(
                None,
                judging(PhantomData::<Polled>, every, every, every),
                |_| async { Ok(40) },
            )
            .await;
        assert!(matches!(polled, Ok(40)), "{polled:?}");
        let answered = lines.expect_one(TARGET, RETRYING_ANSWERED);
        assert_eq!(
            (
                answered.level.as_str(),
                answered.field("attempt").as_deref(),
                answered.field("error")
            ),
            ("debug", Some("1"), None),
            "{answered:?}"
        );
        lines.expect_none(TARGET, RETRYING_FAILED);
    }

    /// The message of each line filed on this crate's target, in the order
    /// they were filed.
    fn filed(lines: &nest_rs_testing::LogCapture) -> Vec<String> {
        lines
            .events()
            .into_iter()
            .filter(|line| line.target == TARGET)
            .map(|line| line.message)
            .collect()
    }

    /// The breaker's transition, the one line of the ten its own file says.
    const BREAKER_MOVED: &str = crate::teller::MOVED;

    /// The breaker said it opened, once: a call it refuses from there files
    /// nothing, wherever it is refused — its run as it opens, under a
    /// bulkhead, or its attempt as it is let through — sent once or again;
    /// nor does the retry a breaker opening under its run refuses, past the
    /// transition and the retry's own line.
    #[tokio::test(start_paused = true)]
    async fn a_call_an_open_breaker_refuses_files_no_line() {
        for concurrency in [None, Some(1)] {
            let policy = policy(ResilienceConfig {
                concurrency,
                ..ResilienceConfig::default()
            });
            policy
                .open_breaker(Duration::from_secs(30))
                .expect("a breaker");
            let lines = nest_rs_testing::LogCapture::install();
            let once = policy.run(answering).await;
            let again = policy.run_idempotent(answering).await;
            assert!(
                matches!(
                    (&once, &again),
                    (
                        Err(ResilienceError::Refused {
                            refusal: ResilienceRefusal::Open { .. },
                            ..
                        }),
                        Err(ResilienceError::Refused {
                            refusal: ResilienceRefusal::Open { .. },
                            ..
                        })
                    )
                ),
                "{once:?}, {again:?}"
            );
            assert_eq!(filed(&lines), [""; 0], "concurrency: {concurrency:?}");
        }

        let policy = policy(ResilienceConfig {
            retries: Some(1),
            breaker_window: Some(1),
            breaker_failures: 1,
            ..ResilienceConfig::default()
        });
        let lines = nest_rs_testing::LogCapture::install();
        let opened = policy
            .run_idempotent(|| async { Err::<(), _>(Refused) })
            .await;
        assert!(
            matches!(
                opened,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                })
            ),
            "{opened:?}"
        );
        assert_eq!(filed(&lines), [BREAKER_MOVED, RETRYING_FAILED]);
    }

    /// A timeout a run ends on is its caller's answer and files nothing:
    /// an attempt given up at its bound, sent once, the last of a call sent
    /// again, whose one line is the retry's, and a unit out of time, refused
    /// as its run opens under a bulkhead or as its attempt is let through.
    #[tokio::test(start_paused = true)]
    async fn a_run_ending_on_a_timeout_files_no_line_of_its_own() {
        let never = || std::future::pending::<Result<(), Refused>>();
        let timed_out = |ran: &Ran| {
            matches!(
                ran,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::TimedOut { .. },
                    ..
                })
            )
        };
        let policy = retrying(1);

        let lines = nest_rs_testing::LogCapture::install();
        let once = policy.run(never).await;
        assert!(timed_out(&once), "{once:?}");
        assert_eq!(filed(&lines), [""; 0]);

        let again = policy.run_idempotent(never).await;
        assert!(timed_out(&again), "{again:?}");
        assert_eq!(
            filed(&lines),
            [RETRYING_FAILED],
            "the retry's, after the first timeout, and none for the one the run ends on"
        );
        drop(lines);

        for concurrency in [None, Some(1)] {
            let policy = self::policy(ResilienceConfig {
                concurrency,
                ..ResilienceConfig::default()
            });
            let lines = nest_rs_testing::LogCapture::install();
            let spent = nest_rs_core::__private::with_deadline(
                Some(tokio::time::Instant::now()),
                policy.run(answering),
            )
            .await;
            assert!(timed_out(&spent), "{spent:?}");
            assert_eq!(filed(&lines), [""; 0], "concurrency: {concurrency:?}");
        }
    }

    /// A call its unit has no time left to send is no attempt, so no retry
    /// of it is given up: refused unsent with retries left, it files no line,
    /// whether a bulkhead refuses its run as it opens or its attempt is
    /// refused as it is let through, and it is told once, as the refusal its
    /// run ends on.
    #[tokio::test(start_paused = true)]
    async fn a_call_its_unit_has_no_time_left_to_send_gives_up_no_retry() {
        for concurrency in [None, Some(4)] {
            let policy = policy(ResilienceConfig {
                retries: Some(2),
                breaker_window: None,
                concurrency,
                ..ResilienceConfig::default()
            });
            let mut events = policy.events();
            let lines = nest_rs_testing::LogCapture::install();
            let calls = AtomicU32::new(0);
            let spent = nest_rs_core::__private::with_deadline(
                Some(tokio::time::Instant::now()),
                policy.run_idempotent(|| async {
                    calls.fetch_add(1, Ordering::Relaxed);
                    Err::<(), _>(Refused)
                }),
            )
            .await;
            assert!(
                matches!(spent, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }) if timeout.is_zero()),
                "{spent:?}"
            );
            assert_eq!(calls.load(Ordering::Relaxed), 0, "nothing was sent");
            assert_eq!(filed(&lines), [""; 0], "concurrency: {concurrency:?}");
            drop(policy);
            let told = events.next().await;
            assert!(
                matches!(told, Some(ResilienceEvent::Timeout { timeout, .. }) if timeout.is_zero()),
                "{told:?}"
            );
            assert_eq!(events.next().await, None, "told once");
        }
    }

    /// The line of a retry the unit's deadline leaves no time for says what
    /// the attempt failed on, and nothing for an `Ok` that was to be sent
    /// again: no error was answered.
    #[tokio::test(start_paused = true)]
    async fn a_retry_of_an_ok_the_deadline_leaves_no_time_for_files_its_line_with_no_error() {
        type Polled = Answer<u8, Refused>;
        let policy = retrying(1);
        let lines = nest_rs_testing::LogCapture::install();
        let polled = nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now() + Duration::from_millis(1)),
            policy.run_again::<Polled, _, Cancellable, _>(
                None,
                judging(PhantomData::<Polled>, every, every, every),
                |_| async { Ok(40) },
            ),
        )
        .await;
        assert!(matches!(polled, Ok(40)), "{polled:?}");
        assert_eq!(filed(&lines), [NOT_ATTEMPTED_AGAIN_PAST_DEADLINE]);
        let given_up = lines.expect_one(TARGET, NOT_ATTEMPTED_AGAIN_PAST_DEADLINE);
        assert_eq!(given_up.field("error"), None, "{given_up:?}");
        assert!(given_up.field("wait_ms").is_some(), "{given_up:?}");
    }

    /// A call the bulkhead refuses files the bulkhead's line and no other,
    /// refused at once or after its wait for a slot.
    #[tokio::test(start_paused = true)]
    async fn a_call_the_bulkhead_refuses_files_its_one_line() {
        for concurrency_wait in [None, Some(Duration::from_secs(1))] {
            let policy = policy(ResilienceConfig {
                concurrency: Some(1),
                concurrency_wait,
                ..ResilienceConfig::default()
            });
            let held = begin(&policy).await.expect("the one slot");
            let lines = nest_rs_testing::LogCapture::install();
            let refused = policy.run(answering).await;
            assert!(
                matches!(
                    refused,
                    Err(ResilienceError::Refused {
                        refusal: ResilienceRefusal::Full,
                        ..
                    })
                ),
                "{refused:?}"
            );
            assert_eq!(filed(&lines), [REFUSED_FULL], "{concurrency_wait:?}");
            drop(held);
        }
    }

    /// A call the rate refuses files the rate's line and no other, refused as
    /// its attempt is let through or, finding no free slot, before it waits
    /// for one.
    #[tokio::test(start_paused = true)]
    async fn a_call_the_rate_refuses_files_its_one_line() {
        let policy = policy(rated(1, Duration::from_secs(60), None));
        assert!(policy.run(answering).await.is_ok());
        let lines = nest_rs_testing::LogCapture::install();
        let refused = policy.run(answering).await;
        assert!(
            matches!(
                refused,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Limited { .. },
                    ..
                })
            ),
            "{refused:?}"
        );
        assert_eq!(filed(&lines), [REFUSED_LIMITED]);
        drop(lines);

        let policy = self::policy(ResilienceConfig {
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(1)),
            ..rated(1, Duration::from_secs(60), None)
        });
        let mut held = begin(&policy).await.expect("the one slot");
        let lines = nest_rs_testing::LogCapture::install();
        let taken = attempt_once(held.as_mut(), answering, judged).await;
        assert!(
            matches!(taken, (Ended::Judged(_), Some(Ok(())))),
            "the bucket's one token"
        );
        let refused = policy.run(answering).await;
        assert!(
            matches!(
                refused,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Limited { .. },
                    ..
                })
            ),
            "{refused:?}"
        );
        assert_eq!(filed(&lines), [REFUSED_LIMITED]);
    }

    /// A unit of work whose transaction refuses to settle the savepoints it
    /// opens, each its own block.
    struct Unsettling;

    impl nest_rs_database::Executor for Unsettling {
        fn holds(&self) -> nest_rs_database::Holds<'_> {
            nest_rs_database::Holds::Transaction(self)
        }
    }

    impl nest_rs_database::HeldTransaction for Unsettling {
        fn after_commit(&self, _work: nest_rs_database::Deferred) {
            unreachable!("no test holds work on this unit")
        }

        fn savepoint(&self) -> Arc<dyn nest_rs_database::AtomicBlock> {
            Arc::new(Self)
        }

        fn outbox(&self) -> Option<&dyn nest_rs_database::Outbox> {
            None
        }
    }

    impl nest_rs_database::AtomicBlock for Unsettling {
        fn settle(
            self: Arc<Self>,
            _ok: bool,
        ) -> Pin<Box<dyn Future<Output = Result<(), AtomicError>> + Send>> {
            Box::pin(std::future::ready(Err(AtomicError::Poisoned {
                retryable: false,
            })))
        }
    }

    /// Every path that files a line, and each refusal that files none, walked
    /// once: the messages filed are the ten the crate's documentation lists,
    /// and no other.
    #[tokio::test(start_paused = true)]
    async fn a_policy_files_its_ten_lines_and_no_other() {
        let lines = nest_rs_testing::LogCapture::install();
        let failing = || async { Err::<(), _>(Refused) };
        let retrying = retrying(1);

        assert!(retrying.run_idempotent(failing).await.is_err());
        type Polled = Answer<u8, Refused>;
        let polled = retrying
            .run_again::<Polled, _, Cancellable, _>(
                None,
                judging(PhantomData::<Polled>, every, every, every),
                |_| async { Ok(40) },
            )
            .await;
        assert!(matches!(polled, Ok(40)), "{polled:?}");
        let asking = retrying
            .run_idempotent(|| async {
                let slow_down = Problem::new(429, nest_rs_core::code::RATE_LIMITED);
                Err::<(), _>(Throttled(slow_down.with_retry_after(3)))
            })
            .await;
        assert!(asking.is_err(), "3 s, past the ceiling of 2 s: {asking:?}");
        let out_of_time = nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now() + Duration::from_millis(100)),
            retrying.run_idempotent(failing),
        )
        .await;
        assert!(out_of_time.is_err(), "{out_of_time:?}");
        let unsettled = nest_rs_database::__private::with_request_executor(
            Arc::new(Unsettling),
            retrying.run_idempotent(answering),
        )
        .await;
        assert!(
            matches!(unsettled, Err(ResilienceError::Unsettled { .. })),
            "{unsettled:?}"
        );
        let never = retrying
            .run(std::future::pending::<Result<(), Refused>>)
            .await;
        assert!(
            matches!(
                never,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::TimedOut { .. },
                    ..
                })
            ),
            "{never:?}"
        );

        let rate = policy(rated(
            1,
            Duration::from_secs(10),
            Some(Duration::from_secs(5)),
        ));
        assert!(rate.run(answering).await.is_ok());
        tokio::time::advance(Duration::from_secs(4)).await;
        let limited = rate.run(answering).await;
        assert!(
            matches!(
                limited,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Limited { .. },
                    ..
                })
            ),
            "{limited:?}"
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(rate.run(answering).await.is_ok(), "its token waited");

        let bulkhead = policy(ResilienceConfig {
            concurrency: Some(1),
            ..ResilienceConfig::default()
        });
        let held = begin(&bulkhead).await.expect("the one slot");
        let full = bulkhead.run(answering).await;
        assert!(
            matches!(
                full,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Full,
                    ..
                })
            ),
            "{full:?}"
        );
        drop(held);

        bulkhead
            .open_breaker(Duration::from_secs(30))
            .expect("a breaker");
        let open = bulkhead.run(answering).await;
        assert!(
            matches!(
                open,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                })
            ),
            "{open:?}"
        );
        let tracking =
            crate::resilient::resilient_method(|| "ShipmentsService", "track", None, None);
        bulkhead.mechanisms.teller.fallback(&tracking, &Refused);

        let filed: std::collections::BTreeSet<_> = filed(&lines).into_iter().collect();
        let listed = [
            BREAKER_MOVED,
            FALLBACK_ANSWERS,
            NOT_ATTEMPTED_AGAIN_PAST_CEILING,
            NOT_ATTEMPTED_AGAIN_PAST_DEADLINE,
            NOT_ATTEMPTED_AGAIN_UNSETTLED,
            REFUSED_FULL,
            REFUSED_LIMITED,
            RETRYING_ANSWERED,
            RETRYING_FAILED,
            WAITING_FOR_TOKEN,
        ];
        assert_eq!(
            filed,
            listed.into_iter().map(str::to_owned).collect(),
            "a refusal by an open breaker and a timeout among them, which file nothing"
        );
    }

    /// A probe answering an `Err` `record_if` refuses is ignored, never
    /// dropped as one its caller gave up: sent once as sent again, the
    /// breaker stays half-open on the one transition its first probe filed,
    /// where a probe dropped on no success yet files it open again.
    #[tokio::test(start_paused = true)]
    async fn a_probe_answering_an_err_record_if_refuses_files_no_transition_and_tells_none() {
        const MOVED: &str = crate::teller::MOVED;
        type Polled = Answer<(), Refused>;
        let policy = policy(ResilienceConfig {
            retries: Some(1),
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(1),
            ..ResilienceConfig::default()
        });
        let failed = policy.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(1)).await;

        let lines = nest_rs_testing::LogCapture::install();
        let mut events = policy.events();
        let once = policy
            .run_once::<Polled, _, Cancellable, _>(
                None,
                judging(PhantomData::<Polled>, every, none, none),
                |_| async { Err(Refused) },
            )
            .await;
        let sent = AtomicU32::new(0);
        let again = policy
            .run_again::<Polled, _, Cancellable, _>(
                None,
                judging(PhantomData::<Polled>, every, none, none),
                |_| async {
                    sent.fetch_add(1, Ordering::Relaxed);
                    Err(Refused)
                },
            )
            .await;
        assert!(
            matches!(
                (&once, &again),
                (
                    Err(ResilienceError::Failed(Refused)),
                    Err(ResilienceError::Failed(Refused))
                )
            ),
            "{once:?}, {again:?}"
        );
        assert_eq!(sent.load(Ordering::Relaxed), 2, "each a probe in its turn");
        assert_eq!(policy.breaker_state(), Some(BreakerState::HalfOpen));

        let filed: Vec<String> = lines
            .find(TARGET, MOVED)
            .iter()
            .map(|line| line.field("state").unwrap_or_default())
            .collect();
        assert_eq!(
            filed,
            ["half_open"],
            "the three probes filed one transition"
        );
        let mut told = Vec::new();
        while let Ok(Some(event)) = tokio::time::timeout(Duration::ZERO, events.next()).await {
            if let ResilienceEvent::Transition { to, .. } = event {
                told.push(to);
            }
        }
        assert_eq!(told, [crate::BreakerPhase::HalfOpen]);
    }

    /// The opening of a breaker says what the attempt that opened it failed
    /// on, run once as run again: its error's whole chain, the timeout's
    /// sentence for one given up at its bound, and for one whose call unwound
    /// that it panicked, never the payload.
    #[tokio::test(start_paused = true)]
    async fn a_breakers_opening_says_what_the_attempt_that_opened_it_failed_on() {
        const PAYLOAD: &str = "the upstream's client panicked";
        let opening = || {
            policy(ResilienceConfig {
                retries: None,
                breaker_window: Some(1),
                breaker_failures: 1,
                ..ResilienceConfig::default()
            })
        };
        let opened = |lines: &nest_rs_testing::LogCapture| {
            let line = lines.expect_one(TARGET, crate::teller::MOVED);
            assert_eq!(line.field("state").as_deref(), Some("open"), "{line:?}");
            line.field("error")
        };

        let lines = nest_rs_testing::LogCapture::install();
        let failed = opening()
            .run(|| async { Err::<(), _>(unreachable()) })
            .await;
        assert!(
            matches!(failed, Err(ResilienceError::Failed(_))),
            "{failed:?}"
        );
        assert_eq!(opened(&lines).as_deref(), Some(UNREACHABLE));
        drop(lines);

        let lines = nest_rs_testing::LogCapture::install();
        let again = opening()
            .run_idempotent(|| async { Err::<(), _>(unreachable()) })
            .await;
        assert!(
            matches!(again, Err(ResilienceError::Failed(_))),
            "{again:?}"
        );
        assert_eq!(opened(&lines).as_deref(), Some(UNREACHABLE));
        drop(lines);

        let lines = nest_rs_testing::LogCapture::install();
        let timed_out = opening()
            .run(std::future::pending::<Result<(), Refused>>)
            .await;
        assert!(
            matches!(
                timed_out,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::TimedOut { .. },
                    ..
                })
            ),
            "{timed_out:?}"
        );
        assert_eq!(opened(&lines).as_deref(), Some(TIMED_OUT));
        drop(lines);

        let lines = nest_rs_testing::LogCapture::install();
        let policy = opening();
        let unwound = nest_rs_core::panic::contain(policy.run(|| async {
            if true {
                panic!("{PAYLOAD}");
            }
            Ok::<_, Refused>(())
        }))
        .await;
        assert!(unwound.is_err(), "the panic unwinds on");
        assert_eq!(opened(&lines).as_deref(), Some(PANICKED));
        assert!(!lines.quotes(PAYLOAD), "{:#?}", lines.events());
    }

    /// An error telling, each time it is said, whether its policy's breaker
    /// was locked then.
    #[derive(Debug)]
    struct Watching {
        policy: Arc<ResiliencePolicy<Upstream>>,
        said: Arc<AtomicU32>,
        under_the_lock: Arc<AtomicU32>,
    }

    impl fmt::Display for Watching {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.said.fetch_add(1, Ordering::Relaxed);
            if self
                .policy
                .mechanisms
                .breaker
                .as_ref()
                .is_some_and(Breaker::locked)
            {
                self.under_the_lock.fetch_add(1, Ordering::Relaxed);
            }
            f.write_str("the upstream refused, watching")
        }
    }

    impl Error for Watching {}

    /// A call's error is user code: it is said where a line hears it — the
    /// retry's, then the breaker's opening — once for each, and never under
    /// the breaker's lock, where an error reading its policy's breaker would
    /// wait for good.
    #[tokio::test(start_paused = true)]
    async fn a_failed_attempts_error_is_said_once_and_never_under_the_breakers_lock() {
        let policy = Arc::new(policy(ResilienceConfig {
            retries: Some(1),
            breaker_window: Some(2),
            breaker_failures: 2,
            ..ResilienceConfig::default()
        }));
        let (said, under_the_lock) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let lines = nest_rs_testing::LogCapture::install();

        let failed = policy
            .run_idempotent(|| async {
                Err::<(), _>(Watching {
                    policy: Arc::clone(&policy),
                    said: Arc::clone(&said),
                    under_the_lock: Arc::clone(&under_the_lock),
                })
            })
            .await;
        assert!(matches!(failed, Err(ResilienceError::Failed(_))));
        let errors: Vec<_> = lines
            .events()
            .iter()
            .filter_map(|line| line.field("error"))
            .collect();
        assert_eq!(
            errors, ["the upstream refused, watching"; 2],
            "the retry's line, then the opening on the second failure"
        );
        assert_eq!(
            (
                said.load(Ordering::Relaxed),
                under_the_lock.load(Ordering::Relaxed)
            ),
            (2, 0),
            "said once an attempt, the breaker free"
        );
    }

    /// An error saying itself by counting how often it is said, and panicking
    /// when told to.
    #[derive(Debug)]
    struct Saying {
        said: Arc<AtomicU32>,
        panics: bool,
    }

    impl fmt::Display for Saying {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.said.fetch_add(1, Ordering::Relaxed);
            assert!(!self.panics, "the error's words panic");
            f.write_str("the upstream refused, saying so")
        }
    }

    impl Error for Saying {}

    /// A failure below the breaker's threshold, with nothing listening, is
    /// never put in words; the one opening the breaker is, once, for its
    /// transition's line.
    #[tokio::test(start_paused = true)]
    async fn a_failure_is_put_in_words_only_where_it_opens_the_breaker() {
        let policy = policy(ResilienceConfig {
            retries: None,
            breaker_window: Some(3),
            breaker_failures: 2,
            ..ResilienceConfig::default()
        });
        let said = Arc::new(AtomicU32::new(0));
        let failing = || {
            policy.run(|| {
                let said = Arc::clone(&said);
                async move {
                    Err::<(), _>(Saying {
                        said,
                        panics: false,
                    })
                }
            })
        };
        assert!(failing().await.is_err());
        assert_eq!(said.load(Ordering::Relaxed), 0, "below the threshold");
        assert!(failing().await.is_err());
        assert_eq!(said.load(Ordering::Relaxed), 1, "said for the opening");
        assert!(matches!(
            policy.breaker_state(),
            Some(BreakerState::Open { .. })
        ));
    }

    /// Words that panic as the failure they say opens the breaker count once,
    /// as a panic of the call's, and unwind out of its run: the breaker is
    /// open, its transition saying the call panicked.
    #[tokio::test(start_paused = true)]
    async fn words_panicking_at_the_opening_count_once_and_unwind() {
        let policy = policy(ResilienceConfig {
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            ..ResilienceConfig::default()
        });
        let said = Arc::new(AtomicU32::new(0));
        let lines = nest_rs_testing::LogCapture::install();
        let run = policy.run(|| {
            let said = Arc::clone(&said);
            async move { Err::<(), _>(Saying { said, panics: true }) }
        });
        let mut run = pin!(run);
        let unwound = catch_unwind(AssertUnwindSafe(|| {
            run.as_mut().poll(&mut Context::from_waker(Waker::noop()))
        }));
        assert!(unwound.is_err(), "the run unwound");
        assert_eq!(said.load(Ordering::Relaxed), 1);
        assert!(matches!(
            policy.breaker_state(),
            Some(BreakerState::Open { .. })
        ));
        let opened = lines.find(TARGET, BREAKER_MOVED);
        assert_eq!(opened.len(), 1, "{opened:?}");
        assert_eq!(opened[0].field("error").as_deref(), Some(PANICKED));
    }

    /// Each retry sent adds one to the retries' counter, and each run ending on
    /// the policy's refusal one to the refusals', by that refusal: a retry the
    /// breaker or the rate refuses, never sent, adds to the refusals' alone, and
    /// so does a run a policy with a bulkhead refuses before taking a slot.
    #[cfg(feature = "opentelemetry")]
    #[tokio::test(start_paused = true)]
    async fn retries_and_the_refusals_runs_end_on_are_counted() {
        use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
        use opentelemetry_sdk::metrics::{
            InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        };

        use crate::counters::{INSTANCE_ATTRIBUTE, REFUSAL_ATTRIBUTE, REFUSALS, RETRIES};

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());

        let failed = retrying(1)
            .run_idempotent(|| async { Err::<(), _>(Refused) })
            .await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));

        let timed_out = policy(ResilienceConfig {
            timeout: Duration::from_millis(100),
            retries: None,
            breaker_window: None,
            ..ResilienceConfig::default()
        })
        .run(std::future::pending::<Result<(), Refused>>)
        .await;
        assert!(matches!(
            timed_out,
            Err(ResilienceError::Refused {
                refusal: ResilienceRefusal::TimedOut { .. },
                ..
            })
        ));

        let opening = policy(ResilienceConfig {
            retries: Some(1),
            breaker_window: Some(1),
            breaker_failures: 1,
            ..ResilienceConfig::default()
        });
        let open = opening
            .run_idempotent(|| async { Err::<(), _>(Refused) })
            .await;
        assert!(
            matches!(
                open,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                })
            ),
            "its retry, refused unsent by the breaker its failure opened: {open:?}"
        );

        let narrow = policy(ResilienceConfig {
            concurrency: Some(1),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        let held = begin(&narrow).await.expect("the one slot");
        let full = narrow.run(|| async { Ok::<_, Refused>(()) }).await;
        assert!(matches!(
            full,
            Err(ResilienceError::Refused {
                refusal: ResilienceRefusal::Full,
                ..
            })
        ));
        drop(held);

        let waiting = policy(ResilienceConfig {
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(60)),
            concurrency_waiters: Some(1),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        let held = begin(&waiting).await.expect("the one slot");
        let mut queued = Box::pin(waiting.run(|| async { Ok::<_, Refused>(()) }));
        park(&mut queued).await;
        let crowded = waiting.run(|| async { Ok::<_, Refused>(()) }).await;
        assert!(
            matches!(
                crowded,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Full,
                    ..
                })
            ),
            "the one place to wait in taken: {crowded:?}"
        );
        drop(held);
        assert!(queued.await.is_ok(), "the waiter took the slot");

        let spent = policy(ResilienceConfig {
            concurrency: Some(1),
            rate_limit: Some(1),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        assert!(spent.run(|| async { Ok::<_, Refused>(()) }).await.is_ok());
        let held = begin(&spent).await.expect("the one slot");
        let read = spent.run(|| async { Ok::<_, Refused>(()) }).await;
        assert!(
            matches!(
                read,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Limited { .. },
                    ..
                })
            ),
            "finding no free slot, refused by the rate it reads first: {read:?}"
        );
        drop(held);

        let rated = policy(ResilienceConfig {
            retries: Some(1),
            rate_limit: Some(1),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        let limited = rated
            .run_idempotent(|| async { Err::<(), _>(Refused) })
            .await;
        assert!(
            matches!(
                limited,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Limited { .. },
                    ..
                })
            ),
            "its retry, refused unsent by the bucket its first attempt emptied: {limited:?}"
        );

        let gated = policy(ResilienceConfig {
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            concurrency: Some(1),
            ..ResilienceConfig::default()
        });
        let spent = nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now()),
            gated.run(|| async { Ok::<_, Refused>(()) }),
        )
        .await;
        assert!(
            matches!(
                spent,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::TimedOut { .. },
                    ..
                })
            ),
            "a unit out of time, refused before the bulkhead: {spent:?}"
        );
        let failed = gated.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        let open = gated.run(|| async { Ok::<_, Refused>(()) }).await;
        assert!(
            matches!(
                open,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                })
            ),
            "the breaker its failure opened, refusing before the bulkhead: {open:?}"
        );

        provider.force_flush().expect("the export");
        let exported = exporter
            .get_finished_metrics()
            .expect("the exported metrics");
        let points = |name: &str| -> Vec<(String, u64)> {
            let mut points: Vec<(String, u64)> = exported
                .iter()
                .flat_map(|resource| resource.scope_metrics())
                .flat_map(|scope| scope.metrics())
                .filter(|metric| metric.name() == name)
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
                            (attribute(REFUSAL_ATTRIBUTE), point.value())
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            points.sort();
            points
        };
        assert_eq!(
            points(RETRIES),
            [(String::new(), 1)],
            "one call sent again; the two retries refused unsent count none"
        );
        assert_eq!(
            points(REFUSALS),
            [
                ("full".to_owned(), 2),
                ("limited".to_owned(), 2),
                ("open".to_owned(), 2),
                ("timed_out".to_owned(), 2),
            ],
            "a failed call is the call's answer, no refusal of the policy's; a full bulkhead and \
             a full place to wait in are one refusal, as a rate read before the slot and one \
             counted are, and as a unit out of time and an open breaker are before a bulkhead \
             and at an attempt"
        );
    }

    /// An open time under a second, set in code, refuses every call for that
    /// long, told what is left of it in a `Retry-After` of a second, rounded
    /// up, then lets the next call probe; a manual opening for one does the
    /// same.
    #[tokio::test(start_paused = true)]
    async fn a_breaker_open_under_a_second_refuses_for_that_long_then_probes() {
        let open = Duration::from_millis(500);
        let policy = pinned(ResilienceConfig {
            retries: None,
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: open,
            ..ResilienceConfig::default()
        });
        let failed = policy.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        let into = Duration::from_millis(200);
        tokio::time::advance(into).await;
        let Err(refused) = policy.run(answering).await else {
            panic!("open for 500 ms");
        };
        let ResilienceError::Refused {
            refusal: ResilienceRefusal::Open { retry_after, .. },
            ..
        } = &refused
        else {
            panic!("refused by the breaker: {refused:?}");
        };
        assert_eq!(*retry_after, open - into);
        let problem = nest_rs_core::__private::find_in_chain::<Problem>(&refused)
            .expect("the problem rides the chain");
        assert_eq!(problem.retry_after(), Some(1), "a second, rounded up");
        tokio::time::advance(open - into).await;
        assert!(policy.run(answering).await.is_ok(), "the probe");
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));

        policy
            .open_breaker(open)
            .expect("an open time `breaker_open` takes");
        tokio::time::advance(open - Duration::from_millis(1)).await;
        assert!(
            matches!(
                policy.run(answering).await,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                })
            ),
            "a millisecond short of the opening's end"
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(policy.run(answering).await.is_ok(), "the probe");
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
    }

    /// An age under a second, set in code, forgets a failure older than it: a
    /// breaker one failure short of opening stays closed on a failure past the
    /// age of the one before, and opens on one within it.
    #[tokio::test(start_paused = true)]
    async fn a_breaker_age_under_a_second_forgets_a_failure_older_than_it() {
        let age = Duration::from_millis(500);
        let policy = pinned(ResilienceConfig {
            retries: None,
            breaker_window: Some(4),
            breaker_failures: 2,
            breaker_age: Some(age),
            ..ResilienceConfig::default()
        });
        let failing = || async { Err::<(), _>(Refused) };
        assert!(policy.run(&failing).await.is_err());
        tokio::time::advance(age + Duration::from_millis(1)).await;
        assert!(policy.run(&failing).await.is_err());
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Closed),
            "the first failure is past the age"
        );
        tokio::time::advance(age).await;
        assert!(policy.run(failing).await.is_err());
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "two failures within the age of one another"
        );
    }

    /// A window under a second, set in code, sends its limit at once and
    /// refuses the next attempt unsent, told the time to the next token in a
    /// `Retry-After` of a second, rounded up; once that token is back, it
    /// sends again.
    #[tokio::test(start_paused = true)]
    async fn a_rate_window_under_a_second_refills_its_bucket_that_fast() {
        let window = Duration::from_millis(500);
        let policy = pinned(rated(2, window, None));
        let calls = AtomicU32::new(0);
        let sent = || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Refused>(())
        };
        assert!(policy.run(&sent).await.is_ok());
        let into = Duration::from_millis(100);
        tokio::time::advance(into).await;
        assert!(policy.run(&sent).await.is_ok());
        let Err(refused) = policy.run(&sent).await else {
            panic!("past the limit");
        };
        let ResilienceError::Refused {
            refusal: ResilienceRefusal::Limited { retry_after, .. },
            ..
        } = &refused
        else {
            panic!("refused by the rate: {refused:?}");
        };
        assert_eq!(
            *retry_after,
            window / 2 - into,
            "the token the first call took is back half a window after it, at 250 ms"
        );
        let problem = nest_rs_core::__private::find_in_chain::<Problem>(&refused)
            .expect("the problem rides the chain");
        assert_eq!(problem.retry_after(), Some(1), "a second, rounded up");
        assert_eq!(
            calls.load(Ordering::Relaxed),
            2,
            "a refused call is never sent"
        );
        tokio::time::advance(window / 2 - into).await;
        assert!(policy.run(sent).await.is_ok(), "the token is back");
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    /// A manual opening refuses every call unsent, a retry included, told what
    /// is left of its time, as an open breaker does; a manual closing sends
    /// them again.
    #[tokio::test(start_paused = true)]
    async fn a_breaker_opened_by_hand_refuses_every_call_unsent_until_it_is_closed_by_hand() {
        let policy = policy(ResilienceConfig::default());
        let calls = AtomicU32::new(0);
        let sent = || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Refused>(())
        };
        policy
            .open_breaker(Duration::from_secs(20))
            .expect("an open time `breaker_open` takes");
        tokio::time::advance(Duration::from_secs(5)).await;
        let refused = policy.run_idempotent(&sent).await;
        assert!(
            matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Open { retry_after, .. }, .. })
                if retry_after == Duration::from_secs(15)),
            "{refused:?}"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 0, "refused unsent");
        policy.close_breaker().expect("a breaker");
        assert!(policy.run(sent).await.is_ok());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    /// A probe holding its turn while it waits for the rate's token reads
    /// half-open, carrying no time, while a call beside it is told when the
    /// probe has answered at the latest. Opened by hand meanwhile, the breaker
    /// refuses the probe when its wait ends, unsent, as it refuses any call
    /// meeting it open, before the rate is asked.
    #[tokio::test(start_paused = true)]
    async fn a_probe_waiting_for_its_token_when_a_manual_opening_comes_is_refused_unsent() {
        let policy = policy(ResilienceConfig {
            timeout: Duration::from_secs(5),
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(1),
            ..rated(1, Duration::from_secs(10), Some(Duration::from_secs(10)))
        });
        let failed = policy.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(1)).await;
        let sent = AtomicU32::new(0);
        let mut probing = Box::pin(policy.run(|| async {
            sent.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Refused>(())
        }));
        park(&mut probing).await;
        assert_eq!(policy.breaker_state(), Some(BreakerState::HalfOpen));
        let beside = policy.run(answering).await;
        assert!(
            matches!(beside, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Open { retry_after, .. }, .. })
                if retry_after == Duration::from_secs(14)),
            "at 1 s, the probe sent at its token's instant, 10 s, within its 5 s timeout: {beside:?}"
        );

        policy
            .open_breaker(Duration::from_secs(30))
            .expect("an open time `breaker_open` takes");
        let refused = probing.await;
        assert!(
            matches!(refused, Err(ResilienceError::Refused { refusal: ResilienceRefusal::Open { retry_after, .. }, .. })
                if retry_after == Duration::from_secs(21)),
            "at 10 s, what is left of the 30 s it was opened for at 1 s: {refused:?}"
        );
        assert_eq!(sent.load(Ordering::Relaxed), 0, "the probe is never sent");
        policy.close_breaker().expect("a breaker");
        let resumed = tokio::time::Instant::now();
        assert!(policy.run(answering).await.is_ok());
        assert_eq!(
            resumed.elapsed(),
            Duration::ZERO,
            "the refused probe gave its token back after its wait"
        );
    }

    /// A probe waiting for the rate's token loses its turn to a manual
    /// opening, which may pass before the wait ends: the call then meets the
    /// breaker as a call arriving then does, takes the probe's turn of the
    /// opening by hand, is sent, and its answer closes the breaker.
    #[tokio::test(start_paused = true)]
    async fn a_probe_waiting_for_its_token_past_a_shorter_manual_opening_probes_after_it() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(ResilienceConfig {
            timeout: Duration::from_secs(5),
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(1),
            ..rated(1, Duration::from_secs(10), Some(Duration::from_secs(10)))
        });
        let failed = policy.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(1)).await;
        let sent = AtomicU32::new(0);
        let mut probing = Box::pin(policy.run(|| async {
            sent.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Refused>(())
        }));
        park(&mut probing).await;
        policy
            .open_breaker(Duration::from_secs(1))
            .expect("an open time `breaker_open` takes");
        let probed = probing.await;
        assert!(
            matches!(probed, Ok(())),
            "at 10 s, past the opening by hand at 1 s for 1 s: {probed:?}"
        );
        assert_eq!(sent.load(Ordering::Relaxed), 1, "sent at its token");
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
        let states: Vec<String> = lines
            .find(TARGET, crate::teller::MOVED)
            .iter()
            .map(|line| line.field("state").unwrap_or_default())
            .collect();
        assert_eq!(
            states,
            ["open", "open", "half_open", "closed"],
            "opened by a failure, then by hand, then probed at its token's instant"
        );
    }

    /// Every event `events` was told and has not answered yet, waiting for none.
    fn heard(events: &mut ResilienceEvents) -> Vec<ResilienceEvent> {
        let mut heard = Vec::new();
        loop {
            // Outside tokio's budget, which would end a long read as if it waited.
            let mut next = pin!(tokio::task::unconstrained(events.next()));
            match next.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
                Poll::Ready(Some(event)) => heard.push(event),
                Poll::Ready(None) | Poll::Pending => return heard,
            }
        }
    }

    /// A token brought forward to a probe waiting for it moves when that
    /// probe has answered at the latest: a call refused beside it is told the
    /// wait to the nearer token, then the probe's bound.
    #[tokio::test(start_paused = true)]
    async fn a_token_brought_forward_to_a_waiting_probe_moves_what_a_refused_call_is_told() {
        let policy = policy(ResilienceConfig {
            rate_limit: Some(1),
            rate_window: Duration::from_secs(10),
            rate_wait: Some(Duration::from_secs(60)),
            ..probing(Duration::from_secs(5))
        });
        assert!(policy.run(answering).await.is_ok());
        let mut ahead = Box::pin(policy.run(answering));
        park(&mut ahead).await;
        policy
            .open_breaker(Duration::from_secs(1))
            .expect("a breaker");
        tokio::time::advance(Duration::from_secs(1)).await;
        let mut probe = Box::pin(policy.run(answering));
        park(&mut probe).await;
        let told = async || match policy.run(answering).await {
            Err(ResilienceError::Refused {
                refusal: ResilienceRefusal::Open { retry_after, .. },
                ..
            }) => retry_after.as_secs(),
            other => panic!("{other:?}"),
        };
        assert_eq!(told().await, 24, "its token at 20 s, then 5 s");
        drop(ahead);
        park(&mut probe).await;
        assert_eq!(told().await, 14, "its token at 10 s, then 5 s");
    }

    /// A policy with no breaker and no retry, each attempt given up past
    /// `timeout`: the one a call to another dependency's policy is made under.
    fn around(timeout: Duration) -> ResiliencePolicy<Upstream> {
        policy(ResilienceConfig {
            timeout,
            retries: None,
            breaker_window: None,
            ..ResilienceConfig::default()
        })
    }

    /// A dependency's policy: each attempt given up past five seconds, sent
    /// once, and two failures of its latest two attempts opening its breaker
    /// for a second.
    fn guarding() -> ResilienceConfig {
        ResilienceConfig {
            timeout: Duration::from_secs(5),
            retries: None,
            breaker_window: Some(2),
            breaker_failures: 2,
            breaker_open: Duration::from_secs(1),
            ..ResilienceConfig::default()
        }
    }

    /// A call to a dependency that never answers.
    fn silent() -> std::future::Pending<Result<(), Refused>> {
        std::future::pending()
    }

    /// The outcomes `policy`'s breaker counts, and the failures among them.
    fn counted(policy: &ResiliencePolicy<Upstream>) -> (usize, usize) {
        let stats = policy.breaker_stats().expect("a breaker");
        (stats.outcomes, stats.failures)
    }

    /// Whether `cut` is the timeout of the policy around, given up past `timeout`.
    fn timed_out_at<T: fmt::Debug, E: fmt::Debug>(
        cut: &Result<T, ResilienceError<E>>,
        timeout: Duration,
    ) -> bool {
        matches!(cut, Err(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout: at, .. }, .. }) if *at == timeout)
    }

    /// An attempt in flight as the timeout of a policy around it drops it has
    /// timed out: its own breaker counts it, sent once or again, under a
    /// unit's deadline or none, and opens on a dependency that never answers
    /// however short the timeout of the policy its caller runs under. No
    /// event tells it and no refusal counts it, no caller being left to
    /// answer: the opening alone is told.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_the_timeout_of_a_policy_around_it_drops_has_timed_out_in_its_own_breaker() {
        let lines = nest_rs_testing::LogCapture::install();
        let short = Duration::from_secs(2);
        let outer = around(short);
        for (idempotent, deadline) in [
            (false, None),
            (true, None),
            (false, Some(60)),
            (true, Some(60)),
        ] {
            let inner = policy(ResilienceConfig {
                retries: Some(1),
                ..guarding()
            });
            let mut events = inner.events();
            for hung in 1..=2 {
                let deadline = deadline.map(|secs| Instant::now() + Duration::from_secs(secs));
                let cut = nest_rs_core::__private::with_deadline(
                    deadline,
                    outer.run(|| async {
                        if idempotent {
                            inner.run_idempotent(silent).await
                        } else {
                            inner.run(silent).await
                        }
                    }),
                )
                .await;
                assert!(timed_out_at(&cut, short), "{cut:?}");
                if hung == 1 {
                    assert_eq!(counted(&inner), (1, 1), "idempotent: {idempotent}");
                }
            }
            assert!(
                matches!(inner.breaker_state(), Some(BreakerState::Open { .. })),
                "idempotent: {idempotent}, deadline: {deadline:?}: {:?}",
                inner.breaker_state()
            );
            assert_eq!(
                heard(&mut events),
                [ResilienceEvent::Transition {
                    from: crate::BreakerPhase::Closed,
                    to: crate::BreakerPhase::Open,
                    manual: false,
                }],
                "the opening, and no timeout of a run no caller waits for"
            );
        }
        let openings = lines.find(TARGET, BREAKER_MOVED);
        assert_eq!(openings.len(), 4);
        for opening in openings {
            assert_eq!(
                (opening.field("failed"), opening.field("error")),
                (Some("2".to_owned()), Some(TIMED_OUT.to_owned())),
                "{opening:?}"
            );
        }
    }

    /// Under one unit's deadline, sooner than either timeout, an attempt and
    /// the one of the policy around it end at the same instant: each counts
    /// once, whichever timer the runtime polls first — the inner attempt's
    /// own, the outer's, or the edge's.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_cut_with_the_one_around_it_at_their_units_deadline_counts_once() {
        let soon = Duration::from_millis(300);
        for edge_first in [false, true] {
            let (outer, inner) = (policy(guarding()), policy(guarding()));
            let nested = outer.run(|| inner.run(silent));
            let deadline = Instant::now() + soon;
            if edge_first {
                let cut = cut_first_at(deadline, nested).await;
                assert!(cut.is_none(), "the edge cut the unit first: {cut:?}");
            } else {
                let cut = nest_rs_core::__private::with_deadline(Some(deadline), nested).await;
                assert!(
                    matches!(
                        cut,
                        Err(ResilienceError::Failed(ResilienceError::Refused { refusal: ResilienceRefusal::TimedOut { timeout, .. }, .. }))
                            if timeout == soon
                    ),
                    "the inner attempt's own timer, polled first: {cut:?}"
                );
            }
            assert_eq!(
                (counted(&outer), counted(&inner)),
                ((1, 1), (1, 1)),
                "the edge first: {edge_first}"
            );
        }
    }

    /// A probe in flight as the timeout of a policy around it drops it has
    /// failed: the breaker opens again for its whole open time, filed as a
    /// failed probe that timed out, rather than handing the turn to the next
    /// call at once as a probe its caller dropped does.
    #[tokio::test(start_paused = true)]
    async fn a_probe_the_timeout_of_a_policy_around_it_drops_has_failed() {
        let lines = nest_rs_testing::LogCapture::install();
        let short = Duration::from_secs(2);
        let outer = around(short);
        let inner = policy(probing(Duration::from_secs(5)));
        probe_due(&inner).await;
        let cut = outer.run(|| inner.run(silent)).await;
        assert!(timed_out_at(&cut, short), "{cut:?}");
        assert_eq!(
            inner.breaker_state(),
            Some(BreakerState::Open {
                retry_after: Duration::from_secs(1)
            }),
            "open again for its whole open time"
        );
        let reopened = lines
            .find(TARGET, BREAKER_MOVED)
            .pop()
            .expect("the probe's transition");
        assert_eq!(
            (
                reopened.field("state"),
                reopened.field("failed"),
                reopened.field("error")
            ),
            (
                Some("open".to_owned()),
                Some("1".to_owned()),
                Some(TIMED_OUT.to_owned())
            ),
            "{reopened:?}"
        );
    }

    /// A run that sent nothing as the timeout of a policy around it drops it
    /// counts nothing and gives back what it holds: its place among the calls
    /// waiting for a slot, the token it reserved and the probe's turn it
    /// waited with, and nothing more than its failed attempt while it backs
    /// off.
    #[tokio::test(start_paused = true)]
    async fn a_run_waiting_as_the_timeout_of_a_policy_around_it_drops_it_counts_nothing() {
        let short = Duration::from_secs(2);
        let outer = around(short);

        let for_a_slot = policy(ResilienceConfig {
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(10)),
            ..guarding()
        });
        let slot = begin(&for_a_slot).await.expect("the one slot");
        let cut = outer.run(|| for_a_slot.run(answering)).await;
        assert!(timed_out_at(&cut, short), "{cut:?}");
        assert_eq!(counted(&for_a_slot), (0, 0));
        assert_eq!(for_a_slot.concurrency_waiting(), Some(0));
        drop(slot);

        let for_a_token = policy(ResilienceConfig {
            rate_limit: Some(1),
            rate_window: Duration::from_secs(10),
            rate_wait: Some(Duration::from_secs(60)),
            ..probing(Duration::from_secs(5))
        });
        let started = Instant::now();
        let failed = for_a_token.run(|| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(1)).await;
        let cut = outer.run(|| for_a_token.run(answering)).await;
        assert!(timed_out_at(&cut, short), "{cut:?}");
        assert_eq!(
            for_a_token.breaker_state(),
            Some(BreakerState::HalfOpen),
            "the probe's turn is free, nothing counted against it"
        );
        let probed = for_a_token.run(answering).await;
        assert!(matches!(probed, Ok(())), "{probed:?}");
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(10),
            "sent at the token the dropped run gave back, the next one"
        );
        assert_eq!(for_a_token.breaker_state(), Some(BreakerState::Closed));

        let backing_off = policy(ResilienceConfig {
            retries: Some(1),
            ..guarding()
        });
        let cut = around(Duration::from_millis(100))
            .run(|| backing_off.run_idempotent(|| async { Err::<(), _>(Refused) }))
            .await;
        assert!(timed_out_at(&cut, Duration::from_millis(100)), "{cut:?}");
        assert_eq!(counted(&backing_off), (1, 1), "its one failed attempt");
    }

    /// The timeout of a policy around drops what its own call holds, and
    /// nothing else: an attempt on a task that call spawned goes on to its
    /// own bound, and an attempt its caller drops afterwards counts for
    /// nothing, as before.
    #[tokio::test(start_paused = true)]
    async fn the_timeout_of_a_policy_around_reaches_what_its_call_holds_and_no_further() {
        let short = Duration::from_secs(2);
        let outer = around(short);
        let inner = Arc::new(policy(guarding()));
        let spawned = Arc::clone(&inner);
        let cut = outer
            .run(|| async move {
                let task = tokio::spawn(async move { spawned.run(silent).await });
                task.await.expect("the task is not cancelled")
            })
            .await;
        assert!(timed_out_at(&cut, short), "{cut:?}");
        assert_eq!(counted(&inner), (0, 0), "the task's attempt goes on");
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert_eq!(counted(&inner), (1, 1), "given up at its own bound, at 5 s");

        let dropped = tokio::time::timeout(short, inner.run(silent)).await;
        assert!(dropped.is_err(), "its caller dropped it");
        assert_eq!(
            counted(&inner),
            (1, 1),
            "a caller's drop counts for nothing"
        );
    }

    /// The attempts of two policies in flight together in a call the timeout
    /// of a third drops have each timed out, as have the ones of two nested
    /// one in the other's — and a call of the same policy nested in its own
    /// attempt is that attempt's: one timeout, counted once.
    #[tokio::test(start_paused = true)]
    async fn every_attempt_in_flight_in_a_call_given_up_at_its_bound_has_timed_out() {
        let short = Duration::from_secs(2);
        let (first, second) = (policy(guarding()), policy(guarding()));
        let cut = around(short)
            .run(|| async {
                let (first, second) = tokio::join!(first.run(silent), second.run(silent));
                first.and(second)
            })
            .await;
        assert!(timed_out_at(&cut, short), "{cut:?}");
        assert_eq!((counted(&first), counted(&second)), ((1, 1), (1, 1)));

        let (middle, innermost) = (policy(guarding()), policy(guarding()));
        let cut = around(short)
            .run(|| middle.run(|| innermost.run(silent)))
            .await;
        assert!(timed_out_at(&cut, short), "{cut:?}");
        assert_eq!((counted(&middle), counted(&innermost)), ((1, 1), (1, 1)));

        let nested = policy(guarding());
        let cut = nested
            .run(|| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                nested.run(silent).await
            })
            .await;
        assert!(timed_out_at(&cut, Duration::from_secs(5)), "{cut:?}");
        assert_eq!(
            counted(&nested),
            (1, 1),
            "a call nested in an attempt of its own policy counts for it, once"
        );
    }

    /// An attempt holding its call, as the middleware sends a request, is cut
    /// by the timeout of a policy around it as a run's is: timed out in its
    /// own breaker.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_holding_its_call_the_timeout_of_a_policy_around_it_drops_has_timed_out() {
        let short = Duration::from_secs(2);
        let inner = policy(guarding());
        let cut = around(short)
            .run(|| async {
                let mut run = begin(&inner).await.expect("the request is let through");
                match attempt_once(run.as_mut(), silent, judged).await {
                    (Ended::Refused(refusal), _) => {
                        Err(ResilienceError::<Refused>::refused(refusal, run.instance()))
                    }
                    _ => Ok(()),
                }
            })
            .await;
        assert!(timed_out_at(&cut, short), "{cut:?}");
        assert_eq!(counted(&inner), (1, 1));
    }

    /// A unit of work whose transaction settles the savepoints it opens after
    /// a wait, as a round trip to the database takes one, or never can.
    struct Settling(Result<Duration, ()>);

    impl nest_rs_database::Executor for Settling {
        fn holds(&self) -> nest_rs_database::Holds<'_> {
            nest_rs_database::Holds::Transaction(self)
        }
    }

    impl nest_rs_database::HeldTransaction for Settling {
        fn after_commit(&self, _work: nest_rs_database::Deferred) {
            unreachable!("no test holds work on this unit")
        }

        fn savepoint(&self) -> Arc<dyn nest_rs_database::AtomicBlock> {
            Arc::new(Self(self.0))
        }

        fn outbox(&self) -> Option<&dyn nest_rs_database::Outbox> {
            None
        }
    }

    impl nest_rs_database::AtomicBlock for Settling {
        fn settle(
            self: Arc<Self>,
            _ok: bool,
        ) -> Pin<Box<dyn Future<Output = Result<(), AtomicError>> + Send>> {
            let settles = self.0;
            Box::pin(async move {
                let Ok(wait) = settles else {
                    return Err(AtomicError::Poisoned { retryable: false });
                };
                tokio::time::sleep(wait).await;
                Ok(())
            })
        }
    }

    /// What a unit and an answer below did, in order.
    type Journal = Arc<std::sync::Mutex<Vec<&'static str>>>;

    /// A unit holding a transaction whose savepoints write how they settled.
    struct Journaled(Journal);

    impl nest_rs_database::Executor for Journaled {
        fn holds(&self) -> nest_rs_database::Holds<'_> {
            nest_rs_database::Holds::Transaction(self)
        }
    }

    impl nest_rs_database::HeldTransaction for Journaled {
        fn after_commit(&self, _work: nest_rs_database::Deferred) {
            unreachable!("no test holds work on this unit")
        }

        fn savepoint(&self) -> Arc<dyn nest_rs_database::AtomicBlock> {
            Arc::new(Self(Arc::clone(&self.0)))
        }

        fn outbox(&self) -> Option<&dyn nest_rs_database::Outbox> {
            None
        }
    }

    impl nest_rs_database::AtomicBlock for Journaled {
        fn settle(
            self: Arc<Self>,
            ok: bool,
        ) -> Pin<Box<dyn Future<Output = Result<(), AtomicError>> + Send>> {
            let saw = if ok { "kept" } else { "rolled back" };
            self.0.lock().expect("unpoisoned").push(saw);
            Box::pin(std::future::ready(Ok(())))
        }
    }

    /// An answer writing that it was let go.
    #[derive(Debug)]
    struct Noted(Journal);

    impl Drop for Noted {
        fn drop(&mut self) {
            self.0.lock().expect("unpoisoned").push("answer let go");
        }
    }

    /// A predicate panicking on the answer it reads.
    fn panics_on(_: &Noted) -> bool {
        panic!("the predicate panicked")
    }

    /// An answer a predicate panicked on is let go before the attempt's
    /// savepoint is rolled back, as any answer that will not be the run's:
    /// whatever it holds of the savepoint is gone before the rollback.
    #[tokio::test(start_paused = true)]
    async fn an_answer_a_predicate_panicked_on_is_let_go_before_its_savepoint_rolls_back() {
        type Answered = Answer<Noted, Refused>;
        let policy = retrying(1);
        let journal = Journal::default();
        let run = policy.run_again::<Answered, _, Cancellable, _>(
            None,
            judging(PhantomData::<Answered>, every, panics_on, every),
            |_| {
                let journal = Arc::clone(&journal);
                async move { Ok(Noted(journal)) }
            },
        );
        let unwound =
            nest_rs_core::panic::contain(nest_rs_database::__private::with_request_executor(
                Arc::new(Journaled(Arc::clone(&journal))),
                run,
            ))
            .await;
        assert!(unwound.is_err(), "the predicate's panic unwinds on");
        assert_eq!(
            *journal.lock().expect("unpoisoned"),
            ["answer let go", "rolled back"]
        );
    }

    /// The run of a call that never answers, in a unit holding `transaction`:
    /// declared idempotent, or sent once by a method declaring a fallback,
    /// each of which opens a savepoint.
    fn silent_in<'p>(
        policy: &'p ResiliencePolicy<Upstream>,
        transaction: Settling,
        idempotent: bool,
    ) -> Pin<Box<dyn Future<Output = Ran> + 'p>> {
        const FALLING_BACK: ResilientMethod =
            crate::resilient::resilient_method(|| "QuotesService", "quote", None, None)
                .with_fallback();
        let run: Pin<Box<dyn Future<Output = Ran> + 'p>> = if idempotent {
            Box::pin(policy.run_idempotent(silent))
        } else {
            Box::pin(policy.run_once::<Ran, _, Cancellable, _>(
                Some(&FALLING_BACK),
                judging_by_default(),
                |_| silent(),
            ))
        };
        Box::pin(nest_rs_database::__private::with_request_executor(
            Arc::new(transaction),
            run,
        ))
    }

    /// An attempt given up at its unit's deadline is counted and told as it
    /// ends, before its savepoint is rolled back: the edge cuts the unit at
    /// that instant and drops the run while it settles, and the timeout the
    /// breaker counted is told by its event all the same, once.
    #[tokio::test(start_paused = true)]
    async fn a_timeout_at_the_units_deadline_is_told_before_its_savepoint_settles() {
        let second = Duration::from_secs(1);
        for idempotent in [false, true] {
            for round_trip in [Duration::ZERO, Duration::from_millis(1)] {
                let policy = policy(guarding());
                let mut events = policy.events();
                let deadline = Instant::now() + second;
                let cut = tokio::time::timeout_at(
                    deadline,
                    nest_rs_core::__private::with_deadline(
                        Some(deadline),
                        silent_in(&policy, Settling(Ok(round_trip)), idempotent),
                    ),
                )
                .await;
                if round_trip.is_zero() {
                    assert!(
                        matches!(&cut, Ok(answer) if timed_out_at(answer, second)),
                        "{cut:?}"
                    );
                } else {
                    assert!(cut.is_err(), "cut while its savepoint settles: {cut:?}");
                }
                assert_eq!(counted(&policy), (1, 1));
                let heard = heard(&mut events);
                assert!(
                    matches!(
                        heard[..],
                        [ResilienceEvent::Timeout { timeout, .. }] if timeout == second
                    ),
                    "idempotent: {idempotent}, settled in {round_trip:?}: {heard:?}"
                );
            }
        }
    }

    /// A savepoint the database cannot settle ends the run on `Unsettled`
    /// though its attempt timed out, a retry left to make or none: that
    /// timeout is the attempt's, counted and told as it ended, no retry is
    /// told, and the run's answer says what its unit holds.
    #[tokio::test(start_paused = true)]
    async fn a_timed_out_attempt_whose_savepoint_cannot_settle_is_told_then_ends_unsettled() {
        for (idempotent, retries) in [(false, None), (true, None), (true, Some(2))] {
            let policy = policy(ResilienceConfig {
                retries,
                ..guarding()
            });
            let mut events = policy.events();
            let unsettled = silent_in(&policy, Settling(Err(())), idempotent).await;
            assert!(
                matches!(unsettled, Err(ResilienceError::Unsettled { .. })),
                "{unsettled:?}"
            );
            assert_eq!(counted(&policy), (1, 1));
            let heard = heard(&mut events);
            assert!(
                matches!(
                    heard[..],
                    [ResilienceEvent::Timeout { timeout, .. }] if timeout == Duration::from_secs(5)
                ),
                "idempotent: {idempotent}, retries: {retries:?}: {heard:?}"
            );
        }
    }

    #[instance]
    struct Between;

    /// A call nested in an attempt of its own policy, directly or through a
    /// call of another policy, shares that attempt's turn of the breaker and
    /// slot while no other call does, the first taking its token, and counts
    /// in that attempt, once. A probe whose call nests one of its own policy
    /// closes the breaker, where each nested call took the probe's turn from
    /// itself and was refused, the breaker opening again forever; a second
    /// nested beside the first is refused as any call beside a probe is.
    #[tokio::test(start_paused = true)]
    async fn the_first_call_nested_in_an_attempt_of_its_own_policy_counts_for_it() {
        let probed = policy(probing(Duration::from_secs(5)));
        let between = ResiliencePolicy::<Between>::new(&ResilienceConfig {
            breaker_window: None,
            ..ResilienceConfig::default()
        })
        .expect("a valid policy");
        probe_due(&probed).await;
        let answered = probed
            .run(|| async {
                between
                    .run(|| async { probed.run(|| async { Ok::<_, Refused>("answered") }).await })
                    .await
            })
            .await;
        assert!(matches!(answered, Ok("answered")), "{answered:?}");
        assert_eq!(
            probed.breaker_state(),
            Some(BreakerState::Closed),
            "the probe nesting its own policy's call closed the breaker"
        );

        probe_due(&probed).await;
        let paged = || async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok::<_, Refused>(())
        };
        let fanned = probed
            .run(|| async { Ok::<_, Refused>(all((0..2).map(|_| probed.run(paged))).await) })
            .await
            .expect("the probe answers what its calls did");
        assert!(
            matches!(
                fanned[..],
                [
                    Ok(()),
                    Err(ResilienceError::Refused {
                        refusal: ResilienceRefusal::Open { .. },
                        ..
                    })
                ]
            ),
            "the first call the probe made probed, the one beside it refused: {fanned:?}"
        );
        assert_eq!(probed.breaker_state(), Some(BreakerState::Closed));

        let walled = policy(ResilienceConfig {
            concurrency: Some(1),
            rate_limit: Some(1),
            rate_window: Duration::from_secs(60),
            breaker_window: Some(2),
            breaker_failures: 2,
            ..ResilienceConfig::default()
        });
        let failed = walled
            .run(|| async { walled.run(|| async { Err::<(), _>(Refused) }).await })
            .await;
        assert!(
            matches!(
                failed,
                Err(ResilienceError::Failed(ResilienceError::Failed(Refused)))
            ),
            "the nested call took no second slot and no second token: {failed:?}"
        );
        assert_eq!(
            counted(&walled),
            (1, 1),
            "its failure counted once, by the attempt it was made in"
        );
    }

    /// Every one of `calls` polled together in one task, as a body fanning
    /// them out polls them, each answer in its place.
    async fn all<F: Future>(calls: impl IntoIterator<Item = F>) -> Vec<F::Output> {
        let mut calls: Vec<_> = calls.into_iter().map(|call| Some(Box::pin(call))).collect();
        let mut answers: Vec<_> = calls.iter().map(|_| None).collect();
        std::future::poll_fn(|cx| {
            let mut pending = false;
            for (call, answer) in calls.iter_mut().zip(&mut answers) {
                if let Some(polled) = call {
                    match polled.as_mut().poll(cx) {
                        Poll::Ready(answered) => {
                            *answer = Some(answered);
                            *call = None;
                        }
                        Poll::Pending => pending = true,
                    }
                }
            }
            if pending {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
        answers
            .into_iter()
            .map(|answer| answer.expect("every call answered"))
            .collect()
    }

    /// Calls fanned out inside one attempt of their own policy are each a
    /// call of it: the attempt shares its slot and its turn of the breaker
    /// with one of them at a time and its token with the first, every other
    /// takes its own and counts its outcome. Twenty pages under a bulkhead of
    /// two and a rate of two send two, read alone or inside one attempt, at
    /// most two at once, each failure counted.
    #[tokio::test(start_paused = true)]
    async fn calls_fanned_out_in_an_attempt_of_their_own_policy_each_meet_its_terms() {
        let partner = policy(ResilienceConfig {
            retries: None,
            concurrency: Some(2),
            concurrency_wait: Some(Duration::from_secs(60)),
            rate_limit: Some(2),
            rate_window: Duration::from_secs(60),
            breaker_window: Some(40),
            breaker_failures: 40,
            ..ResilienceConfig::default()
        });
        let (sent, running, most) = (AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0));
        let (sent, running, most) = (&sent, &running, &most);
        let page = move || async move {
            sent.fetch_add(1, Ordering::Relaxed);
            let now = running.fetch_add(1, Ordering::Relaxed) + 1;
            most.fetch_max(now, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(10)).await;
            running.fetch_sub(1, Ordering::Relaxed);
            Err::<(), _>(Refused)
        };
        let pages = || all((0..20).map(|_| partner.run(page)));
        let read = |answered: &[Result<(), ResilienceError<Refused>>]| {
            let failed = answered
                .iter()
                .filter(|answer| matches!(answer, Err(ResilienceError::Failed(Refused))))
                .count();
            (
                failed,
                sent.swap(0, Ordering::Relaxed),
                most.swap(0, Ordering::Relaxed),
            )
        };

        let alone = pages().await;
        assert_eq!(read(&alone), (2, 2, 2), "two sent, at most two at once");
        assert_eq!(counted(&partner), (2, 2));

        tokio::time::advance(Duration::from_secs(60)).await;
        let imported = partner
            .run(|| async { Ok::<_, Refused>(pages().await) })
            .await
            .expect("the import answers what its pages did");
        assert_eq!(
            read(&imported),
            (2, 2, 2),
            "inside one attempt as alone: two sent, at most two at once"
        );
        assert_eq!(
            counted(&partner),
            (4, 4),
            "each page counted, the import failed by the first it made"
        );
        assert_eq!(partner.concurrency_running(), Some(0), "every slot freed");
    }

    /// An attempt holds its slot until it answers: a call of its policy it
    /// makes shares that slot while it runs and gives it back as it ends, so
    /// what the attempt does after it still runs inside the bulkhead. Three
    /// pushes under a bulkhead of one, each fetching a token of the policy's
    /// first, run one at a time; and pages an attempt fans out under a bulkhead
    /// of one with a wait are read one at a time on its slot, each handed it
    /// as the one before gives it back.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_holds_its_slot_past_a_call_of_its_policy_it_made() {
        let partner = policy(ResilienceConfig {
            retries: None,
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(60)),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        let (running, most) = (AtomicU32::new(0), AtomicU32::new(0));
        let (running, most, partner) = (&running, &most, &partner);
        let in_flight = move || async move {
            let now = running.fetch_add(1, Ordering::Relaxed) + 1;
            most.fetch_max(now, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(10)).await;
            running.fetch_sub(1, Ordering::Relaxed);
            Ok::<_, Refused>(())
        };
        let push = move || async move {
            partner
                .run(|| async { Ok::<_, Refused>("token") })
                .await
                .map_err(|_refused| Refused)?;
            in_flight().await
        };
        let pushed = all((0..3).map(|_| partner.run(push))).await;
        assert!(pushed.iter().all(Result::is_ok), "{pushed:?}");
        assert_eq!(
            most.swap(0, Ordering::Relaxed),
            1,
            "one push in flight at once, its token fetched first"
        );
        assert_eq!(partner.concurrency_running(), Some(0), "every slot freed");

        let imported = partner
            .run(|| async { Ok::<_, Refused>(all((0..5).map(|_| partner.run(in_flight))).await) })
            .await
            .expect("the import answers what its pages did");
        assert!(imported.iter().all(Result::is_ok), "{imported:?}");
        assert_eq!(most.load(Ordering::Relaxed), 1, "one page read at a time");
        assert_eq!(partner.concurrency_running(), Some(0), "every slot freed");
    }

    /// Every one of `calls` polled together in one task, each polled again
    /// only once its own waker is woken, as `FuturesUnordered` and
    /// `buffer_unordered` poll them: a call nothing wakes is never polled
    /// again.
    async fn unordered<F: Future>(calls: impl IntoIterator<Item = F>) -> Vec<F::Output> {
        /// One call's waker: it marks the call woken and wakes the task.
        struct OwnWaker {
            woken: std::sync::atomic::AtomicBool,
            task: std::sync::Mutex<Option<Waker>>,
        }
        impl std::task::Wake for OwnWaker {
            fn wake(self: Arc<Self>) {
                self.wake_by_ref();
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.woken.store(true, Ordering::Relaxed);
                if let Some(task) = self.task.lock().expect("unpoisoned").take() {
                    task.wake();
                }
            }
        }
        let mut calls: Vec<_> = calls
            .into_iter()
            .map(|call| {
                let woken = Arc::new(OwnWaker {
                    woken: std::sync::atomic::AtomicBool::new(true),
                    task: std::sync::Mutex::new(None),
                });
                (Some(Box::pin(call)), woken)
            })
            .collect();
        let mut answers: Vec<_> = calls.iter().map(|_| None).collect();
        std::future::poll_fn(|cx| {
            let mut pending = false;
            for ((call, woken), answer) in calls.iter_mut().zip(&mut answers) {
                let Some(polled) = call else { continue };
                *woken.task.lock().expect("unpoisoned") = Some(cx.waker().clone());
                if !woken.woken.swap(false, Ordering::Relaxed) {
                    pending = true;
                    continue;
                }
                let waker = Waker::from(Arc::clone(woken));
                match polled.as_mut().poll(&mut Context::from_waker(&waker)) {
                    Poll::Ready(answered) => {
                        *answer = Some(answered);
                        *call = None;
                    }
                    Poll::Pending => pending = true,
                }
            }
            if pending {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
        answers
            .into_iter()
            .map(|answer| answer.expect("every call answered"))
            .collect()
    }

    /// A call waiting for its attempt's slot is woken as the call sharing it
    /// gives it back: pages an import fans out unordered under a bulkhead of
    /// one with a wait each read in turn on the attempt's slot, well inside
    /// the wait, where a lost wake left them waiting it out and refused
    /// `Full`.
    #[tokio::test(start_paused = true)]
    async fn calls_waiting_for_their_attempts_slot_are_woken_as_it_comes_back() {
        const WAIT: Duration = Duration::from_secs(5);
        let partner = policy(ResilienceConfig {
            retries: None,
            concurrency: Some(1),
            concurrency_wait: Some(WAIT),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        let page = || async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok::<_, Refused>(())
        };
        let started = Instant::now();
        let imported = partner
            .run(|| async { Ok::<_, Refused>(unordered((0..5).map(|_| partner.run(page))).await) })
            .await
            .expect("the import answers what its pages did");
        assert!(imported.iter().all(Result::is_ok), "{imported:?}");
        assert!(
            started.elapsed() < WAIT / 10,
            "each page woken as the one before gave the slot back: {:?}",
            started.elapsed()
        );
        assert_eq!(partner.concurrency_running(), Some(0), "every slot freed");
    }

    /// A call made while another shares the attempt's slot waits for it only
    /// within the bulkhead's own wait: with none, the default, it is refused
    /// `Full` at once, as the same call made alone is.
    #[tokio::test(start_paused = true)]
    async fn a_call_made_while_another_shares_its_attempts_slot_is_refused_full_with_no_wait() {
        let partner = policy(ResilienceConfig {
            retries: None,
            concurrency: Some(1),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        let page = || async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok::<_, Refused>(())
        };
        let imported = partner
            .run(|| async { Ok::<_, Refused>(all((0..3).map(|_| partner.run(page))).await) })
            .await
            .expect("the import answers what its pages did");
        assert!(
            matches!(
                imported[..],
                [
                    Ok(()),
                    Err(ResilienceError::Refused {
                        refusal: ResilienceRefusal::Full,
                        ..
                    }),
                    Err(ResilienceError::Refused {
                        refusal: ResilienceRefusal::Full,
                        ..
                    })
                ]
            ),
            "the first shared the slot, the others found none and no wait: {imported:?}"
        );
        assert_eq!(partner.concurrency_running(), Some(0), "every slot freed");
    }

    /// A call sharing a probe's turn and refused before it is sent gives the
    /// turn back at once: the next call the probe makes shares it and is sent,
    /// where it met the half-open breaker and was refused `Open`.
    #[tokio::test(start_paused = true)]
    async fn a_call_refused_unsent_on_a_probes_turn_gives_the_turn_back() {
        let probed = policy(ResilienceConfig {
            rate_limit: Some(1),
            rate_window: Duration::from_secs(1),
            ..probing(Duration::from_secs(5))
        });
        probe_due(&probed).await;
        let read = || probed.run(|| async { Ok::<_, Refused>(()) });
        let answered = probed
            .run(|| async {
                let token = read().await;
                let limited = read().await;
                tokio::time::sleep(Duration::from_secs(1)).await;
                Ok::<_, Refused>([token, limited, read().await])
            })
            .await
            .expect("the probe answers what its calls did");
        assert!(
            matches!(
                answered[..],
                [
                    Ok(()),
                    Err(ResilienceError::Refused {
                        refusal: ResilienceRefusal::Limited { .. },
                        ..
                    }),
                    Ok(())
                ]
            ),
            "the first took the probe's token, the second found none, the third \
             shared the turn the second gave back: {answered:?}"
        );
        assert_eq!(probed.breaker_state(), Some(BreakerState::Closed));
    }

    /// A call that waited for its token shares its probe's turn as it is let
    /// through, as one that took its token at once does: a probe's second
    /// helper, waiting out the rate's window, is sent on the probe's turn and
    /// the probe closes the breaker, where asking the half-open breaker for a
    /// pass of its own refused it `Open`.
    #[tokio::test(start_paused = true)]
    async fn a_call_that_waited_for_its_token_shares_its_probes_turn() {
        let probed = policy(ResilienceConfig {
            rate_limit: Some(1),
            rate_window: Duration::from_secs(1),
            rate_wait: Some(Duration::from_secs(5)),
            ..probing(Duration::from_secs(5))
        });
        probe_due(&probed).await;
        let read = || probed.run(|| async { Ok::<_, Refused>(()) });
        let started = Instant::now();
        let answered = probed
            .run(|| async { Ok::<_, Refused>([read().await, read().await]) })
            .await
            .expect("the probe answers what its calls did");
        assert!(
            matches!(answered[..], [Ok(()), Ok(())]),
            "the first took the probe's token, the second waited for its own \
             and shared the probe's turn: {answered:?}"
        );
        assert!(
            started.elapsed() >= Duration::from_secs(1),
            "the second waited for its token: {:?}",
            started.elapsed()
        );
        assert_eq!(probed.breaker_state(), Some(BreakerState::Closed));
    }

    /// A call three runs of its policy deep shares the slot its attempt
    /// shares, the shape of a decorated method calling a decorated helper whose
    /// request goes through the policy's middleware: it answers under a
    /// bulkhead of one with no wait, where the innermost call, finding no slot
    /// to share, was refused `Full`.
    #[tokio::test(start_paused = true)]
    async fn a_call_three_deep_shares_the_slot_its_attempt_shares() {
        let partner = policy(ResilienceConfig {
            retries: None,
            concurrency: Some(1),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        let partner = &partner;
        let charged = partner
            .run(|| async {
                partner
                    .run(|| async { partner.run(|| async { Ok::<_, Refused>("token") }).await })
                    .await
            })
            .await;
        assert!(
            matches!(charged, Ok("token")),
            "each call shared the one slot: {charged:?}"
        );
        assert_eq!(partner.concurrency_running(), Some(0), "every slot freed");
    }

    /// An attempt counts once in the breaker, failed when its own answer or a
    /// call of its policy sharing its turn failed: a charge whose token came
    /// back but which failed opens a breaker of one failure, and a probe so
    /// opens the breaker again, where the token's success alone was counted.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_whose_call_of_its_policy_succeeded_counts_its_own_failure() {
        let billing = policy(probing(Duration::from_secs(5)));
        let charge = || async {
            billing
                .run(|| async { Ok::<_, Refused>("token") })
                .await
                .map_err(|_refused| Refused)?;
            Err::<(), _>(Refused)
        };
        let charged = billing.run(charge).await;
        assert!(
            matches!(charged, Err(ResilienceError::Failed(Refused))),
            "{charged:?}"
        );
        assert!(
            matches!(billing.breaker_state(), Some(BreakerState::Open { .. })),
            "the charge's own failure counted: {:?}",
            billing.breaker_state()
        );

        tokio::time::advance(Duration::from_secs(1)).await;
        let probed = billing.run(charge).await;
        assert!(
            matches!(probed, Err(ResilienceError::Failed(Refused))),
            "{probed:?}"
        );
        assert!(
            matches!(billing.breaker_state(), Some(BreakerState::Open { .. })),
            "the probe failed, its token's success notwithstanding: {:?}",
            billing.breaker_state()
        );

        tokio::time::advance(Duration::from_secs(1)).await;
        let recovered = billing
            .run(|| async {
                billing.run(|| async { Err::<(), _>(Refused) }).await.ok();
                Ok::<_, Refused>(())
            })
            .await;
        assert!(recovered.is_ok(), "{recovered:?}");
        assert!(
            matches!(billing.breaker_state(), Some(BreakerState::Open { .. })),
            "a call sharing the probe's turn failed: the probe failed"
        );
    }

    /// A run holds its slot through its retries, a call of its policy one of
    /// its attempts made having shared it: every attempt of a run is sent
    /// holding a slot.
    #[tokio::test(start_paused = true)]
    async fn a_run_holds_its_slot_through_its_retries_once_a_call_of_its_policy_shared_it() {
        let policy = policy(ResilienceConfig {
            retries: Some(1),
            concurrency: Some(1),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        let (attempts, seen) = (AtomicU32::new(0), std::sync::Mutex::new(Vec::new()));
        let (attempts, seen, partner) = (&attempts, &seen, &policy);
        let answered = policy
            .run_idempotent(move || async move {
                let attempt = attempts.fetch_add(1, Ordering::Relaxed);
                seen.lock()
                    .expect("unpoisoned")
                    .push(partner.concurrency_running());
                if attempt == 0 {
                    partner.run(|| async { Err::<(), _>(Refused) }).await
                } else {
                    Ok(())
                }
            })
            .await;
        assert!(answered.is_ok(), "{answered:?}");
        assert_eq!(
            *seen.lock().expect("unpoisoned"),
            [Some(1), Some(1)],
            "each attempt sent holding a slot"
        );
        assert_eq!(policy.concurrency_running(), Some(0), "its slot freed");
    }

    /// An idempotent call made inside an attempt its policy never sends again
    /// — a run once, or the last attempt of a run sent again — is sent again
    /// as its own retries say: nothing around it multiplies them, so nothing
    /// takes them away. Inside an attempt still to be sent again it is sent
    /// once, and a call nested in that one too.
    #[tokio::test(start_paused = true)]
    async fn an_idempotent_call_nested_in_an_attempt_never_sent_again_is_sent_again() {
        let policy = policy(ResilienceConfig {
            retries: Some(2),
            breaker_window: Some(10),
            breaker_failures: 10,
            ..ResilienceConfig::default()
        });
        let calls = AtomicU32::new(0);
        let failing = || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Err::<(), _>(Refused)
        };

        let once = policy.run(|| policy.run_idempotent(failing)).await;
        assert!(
            matches!(
                once,
                Err(ResilienceError::Failed(ResilienceError::Failed(Refused)))
            ),
            "{once:?}"
        );
        assert_eq!(
            calls.swap(0, Ordering::Relaxed),
            3,
            "inside a run once, sent again twice"
        );
        assert_eq!(counted(&policy), (1, 1), "counted once, by the run once");

        let again = policy
            .run_idempotent(|| policy.run_idempotent(failing))
            .await;
        assert!(again.is_err(), "{again:?}");
        assert_eq!(
            calls.swap(0, Ordering::Relaxed),
            1 + 1 + 3,
            "once in each attempt sent again, then twice again in the last"
        );

        let deeper = policy
            .run_idempotent(|| policy.run_idempotent(|| policy.run_idempotent(failing)))
            .await;
        assert!(deeper.is_err(), "{deeper:?}");
        assert_eq!(
            calls.swap(0, Ordering::Relaxed),
            1 + 1 + (1 + 1 + 3),
            "once while any attempt around is still to be sent again"
        );
    }

    /// Whether an idempotent call nested in an attempt that may still be sent
    /// again is sent again is decided as it opens, not once that attempt
    /// answers: an attempt answering `Ok` over the call's error is not sent
    /// again, and the call was sent once, where alone it is sent three times.
    #[tokio::test(start_paused = true)]
    async fn a_nested_idempotent_call_is_sent_once_whatever_its_attempt_then_does() {
        let policy = policy(ResilienceConfig {
            retries: Some(2),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        let calls = AtomicU32::new(0);
        let failing = || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Err::<(), _>(Refused)
        };
        let swallowed = policy
            .run_idempotent(|| async {
                let _ignored = policy.run_idempotent(failing).await;
                Ok::<_, Refused>(())
            })
            .await;
        assert!(swallowed.is_ok(), "{swallowed:?}");
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "sent once, its attempt answering and so sent no more"
        );
    }

    /// A call of its policy an attempt makes fails that attempt only when it
    /// ends on a failure: a token fetch inside a run once that answers on its
    /// retry fails none, so three charges answering `Ok` leave a breaker of
    /// three failures closed, each counted once, a success, and a probe so
    /// shaped closes the breaker. A call ending on its failure after one that
    /// recovered still fails the attempt.
    #[tokio::test(start_paused = true)]
    async fn a_nested_call_answering_on_its_retry_fails_no_attempt_around_it() {
        let partner = policy(ResilienceConfig {
            retries: Some(2),
            breaker_window: Some(3),
            breaker_failures: 3,
            breaker_open: Duration::from_secs(60),
            ..ResilienceConfig::default()
        });
        let tries = AtomicU32::new(0);
        let (partner, tries) = (&partner, &tries);
        let token = || async move {
            tries.store(0, Ordering::Relaxed);
            partner
                .run_idempotent(|| async {
                    if tries.fetch_add(1, Ordering::Relaxed) == 0 {
                        Err(Refused)
                    } else {
                        Ok("token")
                    }
                })
                .await
                .map_err(|_failed| Refused)
        };

        for _ in 0..3 {
            let charged = partner.run(token).await;
            assert!(matches!(charged, Ok("token")), "{charged:?}");
            assert_eq!(
                tries.load(Ordering::Relaxed),
                2,
                "the token sent again once"
            );
        }
        assert_eq!(partner.breaker_state(), Some(BreakerState::Closed));
        assert_eq!(
            counted(partner),
            (3, 0),
            "each charge counted once, a success"
        );

        partner
            .open_breaker(Duration::from_secs(1))
            .expect("a breaker");
        tokio::time::advance(Duration::from_secs(1)).await;
        let probed = partner.run(token).await;
        assert!(matches!(probed, Ok("token")), "{probed:?}");
        assert_eq!(
            partner.breaker_state(),
            Some(BreakerState::Closed),
            "the probe answered, its token on a retry"
        );

        let swallowed = partner
            .run(|| async {
                let fetched = token().await;
                partner.run(|| async { Err::<(), _>(Refused) }).await.ok();
                fetched
            })
            .await;
        assert!(matches!(swallowed, Ok("token")), "{swallowed:?}");
        assert_eq!(
            counted(partner).1,
            1,
            "a call sharing the turn ended on its failure: the attempt failed"
        );

        let past_ceiling = partner
            .run(|| async {
                partner
                    .run_idempotent(|| async {
                        Err::<(), _>(Throttled(
                            Problem::new(429, nest_rs_core::code::RATE_LIMITED)
                                .with_retry_after(3600),
                        ))
                    })
                    .await
                    .ok();
                Ok::<_, Refused>(())
            })
            .await;
        assert!(past_ceiling.is_ok(), "{past_ceiling:?}");
        assert_eq!(
            counted(partner).1,
            2,
            "a call whose retry was given up ended on the failure before it"
        );
    }

    /// A nested call failing twice before it answers owes the attempt around
    /// it no failure: the second failure replaces the first, forgiven, so a
    /// charge answering `Ok` leaves a breaker of one failure closed.
    #[tokio::test(start_paused = true)]
    async fn a_nested_call_answering_on_its_second_retry_fails_no_attempt_around_it() {
        let partner = policy(ResilienceConfig {
            retries: Some(2),
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(60),
            ..ResilienceConfig::default()
        });
        let tries = AtomicU32::new(0);
        let charged = partner
            .run(|| async {
                partner
                    .run_idempotent(|| async {
                        if tries.fetch_add(1, Ordering::Relaxed) < 2 {
                            Err(Refused)
                        } else {
                            Ok("token")
                        }
                    })
                    .await
                    .map_err(|_failed| Refused)
            })
            .await;
        assert!(matches!(charged, Ok("token")), "{charged:?}");
        assert_eq!(
            tries.load(Ordering::Relaxed),
            3,
            "the token sent twice again"
        );
        assert_eq!(
            counted(&partner),
            (1, 0),
            "the charge counted once, a success"
        );
        assert_eq!(partner.breaker_state(), Some(BreakerState::Closed));
    }

    /// A nested call timing out on its own timeout, then answering on its
    /// retry inside an attempt of a longer one, owes that attempt no failure:
    /// a timeout sent again past is owed as a failed verdict is.
    #[tokio::test(start_paused = true)]
    async fn a_nested_call_answering_after_a_timeout_fails_no_attempt_around_it() {
        const CHARGING: ResilientMethod =
            crate::resilient::resilient_method(|| "BillingService", "charge", Some(30_000), None);
        let partner = policy(ResilienceConfig {
            retries: Some(2),
            timeout: Duration::from_millis(100),
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(60),
            ..ResilienceConfig::default()
        });
        let tries = AtomicU32::new(0);
        let charged = partner
            .run_once::<Ran, _, Cancellable, _>(Some(&CHARGING), judging_by_default(), |_| async {
                partner
                    .run_idempotent(|| async {
                        if tries.fetch_add(1, Ordering::Relaxed) == 0 {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                        Ok::<_, Refused>(())
                    })
                    .await
                    .map_err(|_failed| Refused)
            })
            .await;
        assert!(matches!(charged, Ok(())), "{charged:?}");
        assert_eq!(
            tries.load(Ordering::Relaxed),
            2,
            "the helper sent again once"
        );
        assert_eq!(
            counted(&partner),
            (1, 0),
            "the charge counted once, a success"
        );
        assert_eq!(partner.breaker_state(), Some(BreakerState::Closed));
    }

    /// A call that waited for its attempt's slot and took it back shares that
    /// attempt's turn, the turn given back before the slot: pages an import
    /// fans out under a bulkhead of one with a wait, all failing, count as the
    /// import's one outcome, a failure, and leave a breaker of three closed.
    #[tokio::test(start_paused = true)]
    async fn a_page_that_waited_for_the_attempts_slot_counts_in_the_attempt() {
        let partner = policy(ResilienceConfig {
            retries: None,
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(5)),
            breaker_window: Some(20),
            breaker_failures: 3,
            ..ResilienceConfig::default()
        });
        let page = || async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Err::<(), _>(Refused)
        };
        let imported = partner
            .run(|| async { Ok::<_, Refused>(unordered((0..5).map(|_| partner.run(page))).await) })
            .await
            .expect("the import answers what its pages did");
        assert!(imported.iter().all(Result::is_err), "{imported:?}");
        assert_eq!(
            counted(&partner),
            (1, 1),
            "every page counted in the import's attempt"
        );
        assert_eq!(partner.breaker_state(), Some(BreakerState::Closed));
    }

    /// What opens a breaker beside an attempt making calls of its policy:
    /// another caller's failure, or a switch by hand, the attempt a probe or
    /// not.
    #[derive(Clone, Copy, Debug)]
    enum Beside {
        Failure,
        Opening,
        OpeningOnAProbe,
        Closing,
    }

    /// A call sharing its attempt's turn of the breaker asks the breaker
    /// nothing only while the breaker is in the epoch that attempt was let
    /// through in. Once it opened beside the attempt — on another caller's
    /// failure, or by hand, the attempt a probe or not — every call the
    /// attempt makes is refused `Open` unsent; once it was closed by hand,
    /// the next is let through on its own and counts its own outcome. A loop
    /// of five pushes, each failure swallowed, sent all five while the
    /// breaker was open.
    #[tokio::test(start_paused = true)]
    async fn a_call_sharing_its_attempts_turn_is_refused_once_the_breaker_opened_beside_it() {
        for beside in [
            Beside::Failure,
            Beside::Opening,
            Beside::OpeningOnAProbe,
            Beside::Closing,
        ] {
            let partner = policy(ResilienceConfig {
                retries: None,
                breaker_window: Some(1),
                breaker_failures: 1,
                breaker_open: Duration::from_secs(60),
                ..ResilienceConfig::default()
            });
            if matches!(beside, Beside::OpeningOnAProbe) {
                partner
                    .open_breaker(Duration::from_secs(1))
                    .expect("a breaker");
                tokio::time::advance(Duration::from_secs(1)).await;
            }
            let sent = AtomicU32::new(0);
            let (opened, opening) = tokio::sync::oneshot::channel::<()>();
            let (partner, sent) = (&partner, &sent);
            let pushing = partner.run(|| async move {
                let mut pushed = Vec::new();
                let mut opening = Some(opening);
                for push in 0..5 {
                    if push == 1 {
                        match beside {
                            Beside::Failure => {
                                if let Some(opening) = opening.take() {
                                    opening.await.map_err(|_closed| Refused)?;
                                }
                            }
                            Beside::Opening | Beside::OpeningOnAProbe => partner
                                .open_breaker(Duration::from_secs(60))
                                .expect("a breaker"),
                            Beside::Closing => partner.close_breaker().expect("a breaker"),
                        }
                    }
                    pushed.push(
                        partner
                            .run(|| async {
                                sent.fetch_add(1, Ordering::Relaxed);
                                Err::<(), _>(Refused)
                            })
                            .await,
                    );
                }
                Ok::<_, Refused>(pushed)
            });
            let failing_beside = async {
                if matches!(beside, Beside::Failure) {
                    let failed = partner.run(|| async { Err::<(), _>(Refused) }).await;
                    assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
                }
                opened.send(()).ok();
            };
            let (pushed, ()) = tokio::join!(pushing, failing_beside);
            let pushed = pushed.expect("the job answers what its pushes did");
            let let_through = if matches!(beside, Beside::Closing) {
                2
            } else {
                1
            };
            assert_eq!(
                sent.load(Ordering::Relaxed),
                u32::try_from(let_through).expect("a few pushes"),
                "{beside:?}: {pushed:?}"
            );
            assert!(
                pushed[..let_through]
                    .iter()
                    .all(|push| matches!(push, Err(ResilienceError::Failed(Refused)))),
                "{beside:?}: {pushed:?}"
            );
            assert!(
                pushed[let_through..].iter().all(|push| matches!(
                    push,
                    Err(ResilienceError::Refused {
                        refusal: ResilienceRefusal::Open { .. },
                        ..
                    })
                )),
                "{beside:?}: every push past the opening refused unsent: {pushed:?}"
            );
        }
    }

    /// An attempt whose call answered within its bound, inside a unit whose
    /// edge cuts it at a deadline, counts as its answer though its run is
    /// dropped past that bound before it is polled again — the edge's timer
    /// polled first, a worker busy — as its run would have read it: a success
    /// closes a probe's series and counts in the window, where it counted as
    /// a timeout. One whose call has not answered by then has timed out.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_answered_within_its_bound_and_dropped_past_it_counts_as_its_answer() {
        for (probing_first, answering) in [(false, true), (true, true), (false, false)] {
            let policy = policy(ResilienceConfig {
                breaker_window: Some(2),
                breaker_failures: 1,
                ..probing(Duration::from_secs(5))
            });
            if probing_first {
                probe_due(&policy).await;
            }
            let (mut answer, answered) = tokio::sync::oneshot::channel::<()>();
            let deadline = Instant::now() + Duration::from_millis(300);
            let mut run = Box::pin(nest_rs_core::__private::with_deadline(
                Some(deadline),
                policy.run(|| async move { answered.await.map_err(|_closed| Refused) }),
            ));
            assert!(
                run.as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
            let _unanswered = if answering {
                std::mem::replace(&mut answer, tokio::sync::oneshot::channel().0)
                    .send(())
                    .expect("the call waits for its answer");
                None
            } else {
                Some(answer)
            };
            tokio::time::advance(Duration::from_millis(400)).await;
            drop(run);
            if answering {
                assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
                assert_eq!(
                    counted(&policy),
                    (usize::from(!probing_first), 0),
                    "probing first: {probing_first}: the answer counted as a success"
                );
            } else {
                assert!(
                    matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
                    "unanswered past its bound, it timed out: {:?}",
                    policy.breaker_state()
                );
            }
        }
    }
}
