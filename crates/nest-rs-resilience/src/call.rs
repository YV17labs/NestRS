//! What a run calls, whatever its type: its closure, the future each attempt
//! makes of it and the answer that future gives, judged as its method says,
//! all behind [`Calling`], the one seam the run polls a call through. A
//! closure taking its attempt's cancellation token is handed one, and one
//! taking none makes none. A release build holds each future where the run
//! is, whatever its size, and the closure there too, but for a call sent once
//! whose closure is past tokio's threshold, which it boxes with the future
//! that closure makes. A debug build boxes both. A run moves neither once it
//! holds it, and drops the closure as it answers.

use std::error::Error;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use tokio_util::sync::{CancellationToken, DropGuard};

use self::placed::{Placed, placed, placed_mut, unplaced};
pub(crate) use self::placed::{PlacedFuture, placed_future, placed_future_mut};
use crate::breaker::Failure;
use crate::resilient::{Judge, ResilientAnswer};
use crate::verdict::{Verdict, asked};

/// Past this size a release build boxes the closure of a call sent once:
/// tokio's own threshold for a future it spawns in a release build, read off
/// tokio 1.54's `BOX_FUTURE_THRESHOLD` (`src/runtime/mod.rs`,
/// `.claude/upstream/tokio.md`).
const BOX_THRESHOLD: usize = 16384;

/// Whether a call sent once boxes its closure `T`, decided as tokio's own
/// `AutoBox` decides for a future it spawns: an associated constant, read once
/// per type, so only the branch taken is built.
struct AutoBox<T>(PhantomData<T>);

impl<T> AutoBox<T> {
    /// `true` if a value of type `T` is larger than [`BOX_THRESHOLD`].
    const SHOULD_BOX: bool = size_of::<T>() > BOX_THRESHOLD;
}

/// How a release build places a value in a run, or a fallback's future in
/// its method: in place.
#[cfg(not(debug_assertions))]
mod placed {
    use std::pin::Pin;

    pub(crate) type Placed<T> = T;
    pub(crate) type PlacedFuture<T> = T;

    pub(crate) const fn placed<T>(value: T) -> Placed<T> {
        value
    }

    pub(crate) const fn placed_mut<T>(placed: &mut Placed<T>) -> &mut T {
        placed
    }

    pub(crate) fn unplaced<T>(placed: Placed<T>) -> T {
        placed
    }

    pub(crate) const fn placed_future<T>(future: T) -> PlacedFuture<T> {
        future
    }

    pub(crate) const fn placed_future_mut<T>(placed: Pin<&mut PlacedFuture<T>>) -> Pin<&mut T> {
        placed
    }
}

/// How a debug build places a value in a run, or a fallback's future in its
/// method: boxed.
// A debug build copies a value's whole type at each move, and a place that may hold
// a large future has its size (`.claude/upstream/rustc.md`).
#[cfg(debug_assertions)]
mod placed {
    use std::pin::Pin;

    pub(crate) type Placed<T> = Box<T>;
    pub(crate) type PlacedFuture<T> = Pin<Box<T>>;

    pub(crate) fn placed<T>(value: T) -> Placed<T> {
        Box::new(value)
    }

    pub(crate) fn placed_mut<T>(placed: &mut Placed<T>) -> &mut T {
        placed
    }

    #[expect(
        clippy::boxed_local,
        reason = "a call leaves its box in the frame making its future, never in the one polling it"
    )]
    pub(crate) fn unplaced<T>(placed: Placed<T>) -> T {
        *placed
    }

    pub(crate) fn placed_future<T>(future: T) -> PlacedFuture<T> {
        Box::pin(future)
    }

    pub(crate) fn placed_future_mut<T>(placed: Pin<&mut PlacedFuture<T>>) -> Pin<&mut T> {
        placed.get_mut().as_mut()
    }
}

/// A run's call as the run polls it, whatever its closure, its future and
/// its answer: the one type a run is compiled for.
pub(crate) trait Calling {
    /// One poll of the attempt's call, made at its first poll — handed a
    /// token, when it takes one, whose guard `given_up` keeps, cancelling it
    /// when the attempt is given up; its answer is kept once it comes, and its
    /// future left in place for [`spend`](Self::spend).
    fn poll_call(
        self: Pin<&mut Self>,
        given_up: &mut Option<DropGuard>,
        cx: &mut Context<'_>,
    ) -> Poll<()>;

    /// The future the attempt made dropped, outside the poll that contains
    /// its panics: a destructor of the call's that panics unwinds on, as it
    /// does with no policy, and is no panic of the call's answer.
    fn spend(self: Pin<&mut Self>);

    /// How the answer kept counts, for a run that sends its call `again`
    /// when it may: a call sent once asks no predicate of a retry. A
    /// predicate that panics unwinds from here.
    fn judge(&self, again: bool) -> Verdict;

    /// What the answer kept failed on, for an answer judged a failure or
    /// sent again in error: read where it is told, never before.
    fn failure(&self) -> Option<Failure<'_>>;

    /// The answer kept let go: its retry is decided, or a predicate judging
    /// it panicked.
    fn let_go(self: Pin<&mut Self>);
}

/// The one future type a run polls its call as inside an attempt's bound and
/// its savepoint, whatever the call's type.
pub(crate) struct Erased<'a, 'c> {
    pub(crate) call: Pin<&'a mut (dyn Calling + 'c)>,
    pub(crate) given_up: &'a mut Option<DropGuard>,
}

impl Future for Erased<'_, '_> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = &mut *self;
        this.call.as_mut().poll_call(this.given_up, cx)
    }
}

/// Where a run's closure and the future an attempt makes of it are held.
pub(crate) trait Placing {
    /// The future an attempt makes.
    type Made: Future;

    /// One poll of the attempt's future, made at its first — handed a token,
    /// when its call takes one, whose guard `given_up` keeps.
    fn poll_made(
        self: Pin<&mut Self>,
        given_up: &mut Option<DropGuard>,
        cx: &mut Context<'_>,
    ) -> Poll<<Self::Made as Future>::Output>;

    /// The attempt's future dropped once it ends.
    fn spend(self: Pin<&mut Self>);

    /// The closure dropped once the run answers, with what it captured.
    fn release(self: Pin<&mut Self>);
}

/// How an attempt makes the future of a call sent once of its closure `F`,
/// consuming it: handing it a cancellation token, or none.
pub(crate) trait MakeOnce<F> {
    /// The future the closure makes.
    type Made: Future;

    /// The future `call` makes — handed a token, when it takes one, whose
    /// guard `given_up` keeps, cancelling it as the attempt is given up.
    fn make_once(call: F, given_up: &mut Option<DropGuard>) -> Self::Made;
}

/// How each attempt of a run makes its call's future of its closure `F`,
/// kept for the next.
pub(crate) trait MakeAgain<F>: MakeOnce<F> {
    /// The future made for one attempt, as [`MakeOnce::make_once`] makes it.
    fn make_again(call: &mut F, given_up: &mut Option<DropGuard>) -> Self::Made;
}

/// A call whose closure takes its attempt's [`CancellationToken`], cancelled
/// as that attempt is given up.
// Markers, never wrappers: a closure wrapped one type deeper makes rustc report a body that is
// not `Send` through every type of the run (`resilient_record_if_shares_an_argument`).
pub(crate) enum Cancellable {}

/// A call whose closure takes no token: none is made for it.
pub(crate) enum Uncancelled {}

impl<F, Fut> MakeOnce<F> for Cancellable
where
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future,
{
    type Made = Fut;

    fn make_once(call: F, given_up: &mut Option<DropGuard>) -> Fut {
        call(token(given_up))
    }
}

impl<F, Fut> MakeAgain<F> for Cancellable
where
    F: FnMut(CancellationToken) -> Fut,
    Fut: Future,
{
    fn make_again(call: &mut F, given_up: &mut Option<DropGuard>) -> Fut {
        call(token(given_up))
    }
}

impl<F, Fut> MakeOnce<F> for Uncancelled
where
    F: FnOnce() -> Fut,
    Fut: Future,
{
    type Made = Fut;

    fn make_once(call: F, _given_up: &mut Option<DropGuard>) -> Fut {
        call()
    }
}

impl<F, Fut> MakeAgain<F> for Uncancelled
where
    F: FnMut() -> Fut,
    Fut: Future,
{
    fn make_again(call: &mut F, _given_up: &mut Option<DropGuard>) -> Fut {
        call()
    }
}

/// A token for the attempt being made, cancelled as `given_up`'s guard is
/// dropped.
fn token(given_up: &mut Option<DropGuard>) -> CancellationToken {
    let token = CancellationToken::new();
    *given_up = Some(token.clone().drop_guard());
    token
}

/// The future `call` makes as `K` makes it, placed where the run holds it.
// Never inlined into the poll that follows: the closure and the future it returns
// are temporaries of the frame calling it, which would stay on the stack, with
// their whole size, while the future runs.
#[inline(never)]
fn made<K: MakeOnce<F>, F>(
    call: Placed<F>,
    given_up: &mut Option<DropGuard>,
) -> PlacedFuture<K::Made> {
    placed_future(K::make_once(unplaced(call), given_up))
}

/// [`made`] for one attempt of a call made again, its closure kept.
#[inline(never)]
fn made_again<K: MakeAgain<F>, F>(
    call: &mut F,
    given_up: &mut Option<DropGuard>,
) -> PlacedFuture<K::Made> {
    placed_future(K::make_again(call, given_up))
}

/// [`made`], boxed whatever the build: a closure past the threshold.
#[inline(never)]
#[expect(
    clippy::boxed_local,
    reason = "the closure leaves its box in this frame, never in the one polling its future"
)]
fn made_boxed<K: MakeOnce<F>, F>(
    call: Box<F>,
    given_up: &mut Option<DropGuard>,
) -> Pin<Box<K::Made>> {
    Box::pin(K::make_once(*call, given_up))
}

pin_project_lite::pin_project! {
    /// A call its run's one attempt makes as `K` makes it, consuming the
    /// closure: the closure until then and the future it made after, sharing
    /// one place. Past the threshold, a release build boxes both, since the
    /// closure leaves its box to be called and a future built beside it to be
    /// moved into place would take its size again; the place they share still
    /// reserves the larger of their two sizes beside the boxes, and is never
    /// written then, so no value of that size is built to fill it.
    pub(crate) struct Once<F, K>
    where
        K: MakeOnce<F>,
    {
        #[pin]
        placed: OncePlaced<F, K::Made>,
        boxed_call: Option<Box<F>>,
        boxed_made: Option<Pin<Box<K::Made>>>,
        making: PhantomData<fn() -> K>,
    }
}

pin_project_lite::pin_project! {
    #[project = OncePlacedProj]
    enum OncePlaced<F, Fut> {
        Call {
            call: Option<Placed<F>>,
        },
        Made {
            #[pin]
            made: PlacedFuture<Fut>,
        },
        Spent,
    }
}

impl<F, K: MakeOnce<F>> Once<F, K> {
    pub(crate) fn new(call: F) -> Self {
        if AutoBox::<F>::SHOULD_BOX {
            Self {
                placed: OncePlaced::Call { call: None },
                boxed_call: Some(Box::new(call)),
                boxed_made: None,
                making: PhantomData,
            }
        } else {
            Self {
                placed: OncePlaced::Call {
                    call: Some(placed(call)),
                },
                boxed_call: None,
                boxed_made: None,
                making: PhantomData,
            }
        }
    }
}

impl<F, K: MakeOnce<F>> Placing for Once<F, K> {
    type Made = K::Made;

    fn poll_made(
        self: Pin<&mut Self>,
        given_up: &mut Option<DropGuard>,
        cx: &mut Context<'_>,
    ) -> Poll<<K::Made as Future>::Output> {
        let mut this = self.project();
        if AutoBox::<F>::SHOULD_BOX {
            if let Some(call) = this.boxed_call.take() {
                *this.boxed_made = Some(made_boxed::<K, F>(call, given_up));
            }
            let Some(made) = this.boxed_made else {
                unreachable!("an attempt polls the future its call made");
            };
            return made.as_mut().poll(cx);
        }
        if let OncePlacedProj::Call { call } = this.placed.as_mut().project() {
            let Some(call) = call.take() else {
                unreachable!("a call made once is made by its run's one attempt");
            };
            let made = made::<K, F>(call, given_up);
            this.placed.set(OncePlaced::Made { made });
        }
        let OncePlacedProj::Made { made } = this.placed.project() else {
            unreachable!("an attempt polls the future its call made");
        };
        placed_future_mut(made).poll(cx)
    }

    fn spend(self: Pin<&mut Self>) {
        let mut this = self.project();
        if AutoBox::<F>::SHOULD_BOX {
            *this.boxed_made = None;
        } else {
            this.placed.set(OncePlaced::Spent);
        }
    }

    fn release(self: Pin<&mut Self>) {
        let this = self.project();
        if AutoBox::<F>::SHOULD_BOX {
            *this.boxed_call = None;
        } else if let OncePlacedProj::Call { call } = this.placed.project() {
            *call = None;
        }
    }
}

pin_project_lite::pin_project! {
    /// A call each attempt of its run makes as `K` makes it, the closure held
    /// throughout and each attempt's future after it. A release build holds
    /// both where the run is, whatever their size: the closure is called where
    /// it is held, and the run's type reserves its size either way, so a box
    /// would buy no stack.
    pub(crate) struct Again<F, K>
    where
        K: MakeOnce<F>,
    {
        call: Option<Placed<F>>,
        #[pin]
        made: Option<PlacedFuture<K::Made>>,
        making: PhantomData<fn() -> K>,
    }
}

impl<F, K: MakeAgain<F>> Again<F, K> {
    pub(crate) fn new(call: F) -> Self {
        Self {
            call: Some(placed(call)),
            made: None,
            making: PhantomData,
        }
    }
}

impl<F, K: MakeAgain<F>> Placing for Again<F, K> {
    type Made = K::Made;

    fn poll_made(
        self: Pin<&mut Self>,
        given_up: &mut Option<DropGuard>,
        cx: &mut Context<'_>,
    ) -> Poll<<K::Made as Future>::Output> {
        let mut this = self.project();
        if this.made.is_none() {
            let Some(call) = this.call.as_mut().map(placed_mut) else {
                unreachable!("a call made again holds its closure until its run answers");
            };
            this.made.set(Some(made_again::<K, F>(call, given_up)));
        }
        let Some(made) = this.made.as_pin_mut() else {
            unreachable!("an attempt polls the future its call made");
        };
        placed_future_mut(made).poll(cx)
    }

    fn spend(self: Pin<&mut Self>) {
        self.project().made.set(None);
    }

    fn release(self: Pin<&mut Self>) {
        *self.project().call = None;
    }
}

/// How the answers of a call answering `R` count.
pub(crate) trait Reads<R: ResilientAnswer> {
    /// How `answer` counts, for a run that sends its call `again` when it may.
    fn read(&self, answer: &Result<R::Ok, R::Err>, again: bool) -> Verdict;

    /// What `answer` failed on, read where it is told.
    fn failure<'a>(&self, answer: &'a Result<R::Ok, R::Err>) -> Option<Failure<'a>>;
}

impl<R, J> Reads<R> for J
where
    R: ResilientAnswer,
    R::Err: Error + 'static,
    J: Judge<R>,
{
    fn read(&self, answer: &Result<R::Ok, R::Err>, again: bool) -> Verdict {
        judged(self, again, answer)
    }

    fn failure<'a>(&self, answer: &'a Result<R::Ok, R::Err>) -> Option<Failure<'a>> {
        answer.as_ref().err().map(|error| Failure::Error(error))
    }
}

/// How one answer counts, on `judging`'s two axes: an `Ok` is a success,
/// sent again when the run may send its call `again` and `judging` retries
/// it; an `Err` is a failure when it records it and counts in neither
/// direction otherwise, and is sent again when the run may and `judging`
/// retries it, after the wait a [`Problem`](nest_rs_core::Problem) in its
/// chain asks, whichever it counts for. A call sent once asks neither retry
/// predicate. No chain is said here: what an `Err` failed on is read off the
/// answer where it is told.
pub(crate) fn judged<R, J>(judging: &J, again: bool, answer: &Result<R::Ok, R::Err>) -> Verdict
where
    R: ResilientAnswer,
    R::Err: Error + 'static,
    J: Judge<R>,
{
    let error = match answer {
        Ok(answer) if again && judging.retries_ok(answer) => return Verdict::Again,
        Ok(_) => return Verdict::Succeeded,
        Err(error) => error,
    };
    match (judging.records(error), again && judging.retries(error)) {
        (true, true) => Verdict::Failed {
            asked: asked(error),
        },
        (true, false) => Verdict::Final,
        (false, true) => Verdict::IgnoredAgain {
            asked: asked(error),
        },
        (false, false) => Verdict::Ignored,
    }
}

pin_project_lite::pin_project! {
    /// A call held in its place `P`, the answer its attempt kept, and what
    /// judges that answer, `J`, as a call answering `R`.
    pub(crate) struct Judged<P, J, R>
    where
        R: ResilientAnswer,
    {
        #[pin]
        place: P,
        answer: Option<Result<R::Ok, R::Err>>,
        judging: J,
    }
}

impl<P, J, R: ResilientAnswer> Judged<P, J, R> {
    pub(crate) const fn new(place: P, judging: J) -> Self {
        Self {
            place,
            answer: None,
            judging,
        }
    }

    /// The answer the last attempt kept, taken.
    pub(crate) fn take(self: Pin<&mut Self>) -> Option<Result<R::Ok, R::Err>> {
        self.project().answer.take()
    }
}

impl<P: Placing, J, R: ResilientAnswer> Judged<P, J, R> {
    /// The closure dropped, with what it captured, once the run answers.
    pub(crate) fn release(self: Pin<&mut Self>) {
        self.project().place.release();
    }
}

impl<P, J, R> Calling for Judged<P, J, R>
where
    P: Placing,
    P::Made: Future<Output = R::Attempt>,
    J: Reads<R>,
    R: ResilientAnswer,
{
    fn poll_call(
        self: Pin<&mut Self>,
        given_up: &mut Option<DropGuard>,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        let this = self.project();
        let answered = ready!(this.place.poll_made(given_up, cx));
        *this.answer = Some(R::split(answered));
        Poll::Ready(())
    }

    fn spend(self: Pin<&mut Self>) {
        self.project().place.spend();
    }

    fn judge(&self, again: bool) -> Verdict {
        let Some(answer) = &self.answer else {
            unreachable!("an attempt is judged once its call answered");
        };
        self.judging.read(answer, again)
    }

    fn failure(&self) -> Option<Failure<'_>> {
        self.answer
            .as_ref()
            .and_then(|answer| self.judging.failure(answer))
    }

    fn let_go(self: Pin<&mut Self>) {
        *self.project().answer = None;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use nest_rs_core::Problem;

    use super::*;
    use crate::error::ResilienceError;
    use crate::resilient::{every, judging, none};

    /// An error passing on the wait its dependency asked, as a `Problem`.
    #[derive(Debug, thiserror::Error)]
    #[error("the upstream asked to slow down")]
    struct Throttled(#[source] Problem);

    /// What a call answering `Result<T, E>` answers under its policy.
    type Answer<T, E> = Result<T, ResilienceError<E>>;

    /// The two axes are independent: `record_if` alone decides whether an
    /// `Err` is a failure or counts in neither direction, `retry_if` and
    /// `retry_ok_if` alone the retry, and the wait a `Problem` in the chain
    /// asks rides an `Err` sent again whichever it counts for. No chain is
    /// said in judging: what an `Err` failed on is its own, read where told.
    #[test]
    fn an_answer_is_judged_on_two_axes_each_by_its_own_predicate() {
        let throttled =
            Throttled(Problem::new(429, nest_rs_core::code::RATE_LIMITED).with_retry_after(7));
        type Quoted = Answer<u32, Throttled>;

        let asked = Some(Duration::from_secs(7));
        let err = Err::<u32, _>(throttled);
        let ok = Ok::<u32, Throttled>(3);
        let again = |retry_if: fn(&Throttled) -> bool,
                     retry_ok_if: fn(&u32) -> bool,
                     record_if: fn(&Throttled) -> bool,
                     answer: &Result<u32, Throttled>| {
            let judging = judging(PhantomData::<Quoted>, retry_if, retry_ok_if, record_if);
            judged::<Quoted, _>(&judging, true, answer)
        };
        let once = |record_if: fn(&Throttled) -> bool, answer: &Result<u32, Throttled>| {
            let judging = judging(PhantomData::<Quoted>, every, none, record_if);
            judged::<Quoted, _>(&judging, false, answer)
        };

        assert_eq!(
            again(every, none, every, &err),
            Verdict::Failed { asked },
            "by default an `Err` is a failure, sent again"
        );
        assert_eq!(
            again(none, none, every, &err),
            Verdict::Final,
            "a failure `retry_if` refuses ends the run"
        );
        assert_eq!(
            again(every, none, none, &err),
            Verdict::IgnoredAgain { asked },
            "an `Err` `record_if` refuses counts in neither direction, still sent again"
        );
        assert_eq!(
            again(none, none, none, &err),
            Verdict::Ignored,
            "an `Err` both refuse is the run's answer, counted in neither direction"
        );
        assert_eq!(
            again(every, none, every, &ok),
            Verdict::Succeeded,
            "by default an `Ok` is the run's answer"
        );
        assert_eq!(
            again(every, every, every, &ok),
            Verdict::Again,
            "an `Ok` `retry_ok_if` takes is a success sent again, with no error to tell"
        );
        assert_eq!(
            again(every, every, none, &ok),
            Verdict::Again,
            "an `Ok` is a success whatever `record_if` says"
        );

        assert_eq!(
            once(every, &err),
            Verdict::Final,
            "sent once, an `Err` is a failure the run ends on"
        );
        assert_eq!(
            once(none, &err),
            Verdict::Ignored,
            "sent once, an `Err` `record_if` refuses counts in neither direction"
        );
        assert_eq!(once(every, &ok), Verdict::Succeeded);
        assert_eq!(once(none, &ok), Verdict::Succeeded);
    }
}
