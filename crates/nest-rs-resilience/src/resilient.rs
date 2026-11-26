//! What `#[resilient]` expands to: the decorated method a run comes from, the
//! probes reading the field it names and what it answers, the type its body is
//! checked as, the runs that body goes through — once or again, handed its
//! attempt's token where its `cancel` names one and made none otherwise — each
//! handed the predicates its keys name, and the fallback answering in place of
//! a run's error.
//!
//! A body stays in the braces it was written in, so what its tail expression
//! leaves drops as its own crate's edition says: in a crate of edition 2021
//! after the body's locals, as it does with no policy. The example is that
//! crate, a local of each body reading, as it drops, whether the lock the
//! tail expression took is still held:
//!
//! ```edition2021
//! use std::sync::atomic::{AtomicBool, Ordering};
//! use std::sync::{Arc, Mutex};
//!
//! use nest_rs_core::{App, injectable, module};
//! use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
//!
//! #[derive(Debug, thiserror::Error)]
//! #[error("the hall did not answer")]
//! pub struct HallDown;
//!
//! struct Publish<'a>(&'a Mutex<u32>, &'a AtomicBool);
//!
//! impl Drop for Publish<'_> {
//!     fn drop(&mut self) {
//!         self.1.store(self.0.try_lock().is_err(), Ordering::Relaxed);
//!     }
//! }
//!
//! fn seated(seats: u32) -> Result<u32, HallDown> {
//!     Ok(seats)
//! }
//!
//! #[injectable]
//! pub struct SeatsService {
//!     #[inject]
//!     hall: Arc<ResiliencePolicy>,
//!     seats: Mutex<u32>,
//!     locked: AtomicBool,
//! }
//!
//! impl SeatsService {
//!     async fn undecorated(&self) -> Result<u32, HallDown> {
//!         let _publish = Publish(&self.seats, &self.locked);
//!         seated(*self.seats.lock().expect("unpoisoned"))
//!     }
//!
//!     #[resilient(hall)]
//!     async fn once(&self) -> Result<u32, ResilienceError<HallDown>> {
//!         let _publish = Publish(&self.seats, &self.locked);
//!         seated(*self.seats.lock().expect("unpoisoned"))
//!     }
//!
//!     #[resilient(hall, idempotent, fallback = unseated, cancel = token)]
//!     async fn again(&self) -> Result<u32, ResilienceError<HallDown>> {
//!         let _publish = Publish(&self.seats, &self.locked);
//!         drop(token);
//!         seated(*self.seats.lock().expect("unpoisoned"))
//!     }
//!
//!     async fn unseated(
//!         &self,
//!         _error: ResilienceError<HallDown>,
//!     ) -> Result<u32, ResilienceError<HallDown>> {
//!         Ok(0)
//!     }
//!
//!     /// Whether the last body's local dropped with the seats still locked.
//!     fn dropped_locked(&self) -> bool {
//!         self.locked.swap(false, Ordering::Relaxed)
//!     }
//! }
//!
//! #[module(imports = [ResilienceModule], providers = [SeatsService])]
//! pub struct SeatsModule;
//!
//! # #[nest_rs_core::main]
//! # async fn main() -> anyhow::Result<()> {
//! let app = App::builder().module::<SeatsModule>().build().await?;
//! let seats = app.container().get::<SeatsService>().expect("provided");
//! seats.undecorated().await?;
//! assert!(seats.dropped_locked());
//! seats.once().await?;
//! assert!(seats.dropped_locked());
//! seats.again().await?;
//! assert!(seats.dropped_locked());
//! # Ok(())
//! # }
//! ```
//!
//! A tail expression borrowing a local of the body is refused there for the
//! same reason, `E0597`, under a policy as with none:
//!
//! ```edition2021,compile_fail,E0597
//! use std::cell::RefCell;
//! use std::sync::Arc;
//!
//! use nest_rs_resilience::{ResilienceError, ResiliencePolicy, resilient};
//!
//! #[derive(Debug, thiserror::Error)]
//! #[error("the hall did not answer")]
//! pub struct HallDown;
//!
//! pub struct Seats {
//!     hall: Arc<ResiliencePolicy>,
//! }
//!
//! impl Seats {
//!     #[resilient(hall)]
//!     pub async fn counted(&self) -> Result<u32, ResilienceError<HallDown>> {
//!         let seats = RefCell::new(4);
//!         Ok(*seats.borrow())
//!     }
//! }
//! ```

use std::error::Error;
use std::future::{Future, Ready, ready};
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use nest_rs_core::Instance;
use tokio_util::sync::CancellationToken;

use crate::call::{Cancellable, PlacedFuture, Uncancelled, placed_future, placed_future_mut};
use crate::error::ResilienceError;
use crate::policy::ResiliencePolicy;

/// The decorated method a run comes from: its host and its name, which the
/// run's lines carry, and what it pins of its policy for its own runs.
#[derive(Clone, Copy)]
pub struct ResilientMethod {
    host: fn() -> &'static str,
    name: &'static str,
    timeout: Option<Duration>,
    retries: Option<u32>,
    fallback: bool,
}

/// The method `#[resilient]` decorates: `host` names its `Self`, `name` is the
/// method's, and each pin is the one it wrote, `None` for the policy's own.
pub const fn resilient_method(
    host: fn() -> &'static str,
    name: &'static str,
    timeout_ms: Option<u64>,
    retries: Option<u32>,
) -> ResilientMethod {
    ResilientMethod {
        host,
        name,
        timeout: match timeout_ms {
            Some(millis) => Some(Duration::from_millis(millis)),
            None => None,
        },
        retries,
        fallback: false,
    }
}

impl ResilientMethod {
    /// The method, declaring a fallback that answers in place of its run's
    /// error: each of its attempts then runs in a savepoint of its unit's
    /// transaction, rolled back before the fallback when it failed.
    #[must_use]
    pub const fn with_fallback(mut self) -> Self {
        self.fallback = true;
        self
    }

    pub(crate) const fn fallback(&self) -> bool {
        self.fallback
    }

    /// The host, as a `mounted` line names it: the type's own name, without
    /// its path.
    pub(crate) fn host(&self) -> String {
        nest_rs_core::__private::shorten((self.host)())
    }

    pub(crate) const fn name(&self) -> &'static str {
        self.name
    }

    pub(crate) const fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    pub(crate) const fn retries(&self) -> Option<u32> {
        self.retries
    }
}

/// The field a `#[resilient]` method names: the policy it runs under.
#[diagnostic::on_unimplemented(
    message = "`#[resilient]` runs the method under the policy its field holds, and this field \
               holds `{Self}`",
    label = "the field holding the policy",
    note = "inject the policy as `Arc<ResiliencePolicy<I>>` under `#[inject]`, and name that field"
)]
pub trait PolicyField {
    /// The instance the policy is named by.
    type Instance: Instance;

    /// The policy.
    fn policy(&self) -> &ResiliencePolicy<Self::Instance>;
}

impl<I: Instance> PolicyField for Arc<ResiliencePolicy<I>> {
    type Instance = I;

    fn policy(&self) -> &ResiliencePolicy<I> {
        self
    }
}

/// The policy `field` holds.
pub fn policy<F: PolicyField>(field: &F) -> &ResiliencePolicy<F::Instance> {
    field.policy()
}

/// What a `#[resilient]` method answers, read by its type: the policy's run
/// of `Self::Attempt`, the answer its body writes.
#[diagnostic::on_unimplemented(
    message = "a `#[resilient]` method answers `Result<T, ResilienceError<E>>`, and this one \
               answers `{Self}`",
    label = "what the method answers",
    note = "the body is written as answering `Result<T, E>`, and the policy's refusal is the \
            `ResilienceError`'s; a method answering an error of its own calls a decorated one \
            and maps its error"
)]
pub trait ResilientAnswer: Sized {
    /// What a successful call answers.
    type Ok;
    /// What a failed call answers.
    type Err;
    /// What the body answers: `Result<Self::Ok, Self::Err>`.
    type Attempt;

    /// The body's answer, as the policy reads it.
    fn split(attempt: Self::Attempt) -> Result<Self::Ok, Self::Err>;

    /// The run's answer, as the method answers it.
    fn answered(ran: Result<Self::Ok, ResilienceError<Self::Err>>) -> Self;

    /// The error the run ended on, or the method's answer as it is when the
    /// run ended on none.
    fn failed(self) -> Result<Self, ResilienceError<Self::Err>>;
}

impl<T, E> ResilientAnswer for Result<T, ResilienceError<E>> {
    type Ok = T;
    type Err = E;
    type Attempt = Result<T, E>;

    fn split(attempt: Result<T, E>) -> Result<T, E> {
        attempt
    }

    fn answered(ran: Self) -> Self {
        ran
    }

    fn failed(self) -> Result<Self, ResilienceError<E>> {
        match self {
            Err(error) => Err(error),
            answered => Ok(answered),
        }
    }
}

/// The error a `#[resilient]` method answers: one whose chain the policy
/// tells, and reads before a retry. That chain is read as one of
/// `dyn Error + 'static`, so the error owns what it says: one borrowing an
/// argument is refused by rustc's own lifetime error at the answer, which no
/// sentence here reaches.
#[diagnostic::on_unimplemented(
    message = "a `#[resilient]` method's error is a `std::error::Error`, and `{Self}` is not one",
    label = "what the method answers",
    note = "the policy tells the error's chain in the line a retry files and in its breaker's \
            opening, and reads it before a retry for the `Retry-After` a `Problem` in it asks",
    note = "give the method an error implementing `std::error::Error`, as a `thiserror` derive \
            does; one that cannot — `anyhow::Error`, a `Box<dyn Error>` — is held in a type of \
            the app's own that does"
)]
pub trait ChainedError: Error + 'static {}

#[diagnostic::do_not_recommend]
impl<E: Error + 'static> ChainedError for E {}

/// What a predicate key is given when its method leaves it out and every
/// value passes: `retry_if`, `record_if` and `fallback_if`.
pub fn every<V: ?Sized>(_: &V) -> bool {
    true
}

/// What a predicate key is given when its method leaves it out and no value
/// passes: `retry_ok_if`.
pub fn none<V: ?Sized>(_: &V) -> bool {
    false
}

/// A key's value has the shape its key takes.
pub struct Proved;

/// A key's value was refused where it is written. Nothing proves it: an impl
/// over it is what rustc settles on where that value is used next, so one
/// refusal is one error, at the key.
pub struct Refused;

/// The shape a predicate key's value has, read off the value — handed `A`,
/// answering `O` — and its proof `W`: only `Fn(&V) -> bool` has the shape a
/// key takes, so any other is refused as one missing trait, in the key's own
/// sentence, where rustc would report a signature it expected.
pub trait Shaped<V, A, O, W>: Fn(&V) -> bool {}

impl<V, F: Fn(&V) -> bool> Shaped<V, &V, bool, Proved> for F {}

/// A shape no value has, which each predicate key's trait has a second impl
/// over.
// With one impl rustc takes it before reading the value, and refuses a function
// of another arity, or no function, in its own words and twice; with two it weighs
// each against the value, finds none, and refuses the value in the key's sentence.
pub trait Unshaped<V>: Fn(&V) -> bool {}

/// What `retry_if` takes: whether an `Err` the method answered is sent again.
/// `S` is the value's own shape and `W` its proof, both inferred.
#[diagnostic::on_unimplemented(
    message = "a `#[resilient]` method's `retry_if` is a `Fn(&{E}) -> bool` over its error, and \
               this is not one",
    label = "what `retry_if` is given",
    note = "it says whether an `Err` the method answered is sent again: a function's path, or a \
            closure such as `|error| error.is_transient()` — a method of `self` is called from \
            one, `|error| self.is_transient(error)`"
)]
pub trait RetryIf<E, S, W>: Fn(&E) -> bool {}

#[diagnostic::do_not_recommend]
impl<E, F, A, O, W> RetryIf<E, (A,), W> for F where F: Fn(A) -> O + Shaped<E, A, O, W> {}

#[diagnostic::do_not_recommend]
impl<E, F> RetryIf<E, Refused, Refused> for F where F: Unshaped<E> {}

/// What `retry_ok_if` takes: whether an `Ok` the method answered is sent
/// again. `S` is the value's own shape and `W` its proof, both inferred.
#[diagnostic::on_unimplemented(
    message = "a `#[resilient]` method's `retry_ok_if` is a `Fn(&{T}) -> bool` over what it \
               answers, and this is not one",
    label = "what `retry_ok_if` is given",
    note = "it says whether an `Ok` the method answered is sent again: a function's path, or a \
            closure such as `|answer| answer.is_pending()` — a method of `self` is called from \
            one, `|answer| self.is_pending(answer)`"
)]
pub trait RetryOkIf<T, S, W>: Fn(&T) -> bool {}

#[diagnostic::do_not_recommend]
impl<T, F, A, O, W> RetryOkIf<T, (A,), W> for F where F: Fn(A) -> O + Shaped<T, A, O, W> {}

#[diagnostic::do_not_recommend]
impl<T, F> RetryOkIf<T, Refused, Refused> for F where F: Unshaped<T> {}

/// What `record_if` takes: whether an `Err` the method answered counts as a
/// failure in the circuit breaker. `S` is the value's own shape and `W` its
/// proof, both inferred.
#[diagnostic::on_unimplemented(
    message = "a `#[resilient]` method's `record_if` is a `Fn(&{E}) -> bool` over its error, and \
               this is not one",
    label = "what `record_if` is given",
    note = "it says whether an `Err` the method answered counts as a failure in the circuit \
            breaker: a function's path, or a closure such as `|error| !error.is_declined()` — a \
            method of `self` is called from one, `|error| !self.is_declined(error)`"
)]
pub trait RecordIf<E, S, W>: Fn(&E) -> bool {}

#[diagnostic::do_not_recommend]
impl<E, F, A, O, W> RecordIf<E, (A,), W> for F where F: Fn(A) -> O + Shaped<E, A, O, W> {}

#[diagnostic::do_not_recommend]
impl<E, F> RecordIf<E, Refused, Refused> for F where F: Unshaped<E> {}

/// What `fallback_if` takes: whether the fallback answers in place of the
/// error the run ended on. `S` is the value's own shape and its proof,
/// inferred.
#[diagnostic::on_unimplemented(
    message = "a `#[resilient]` method's `fallback_if` is a `Fn(&ResilienceError<{E}>) -> bool` \
               over the error its run ended on, and this is not one",
    label = "what `fallback_if` is given",
    note = "it says whether the fallback answers in place of that error, the call's own or the \
            policy's refusal: a function's path, or a closure such as \
            `|error| matches!(error, ResilienceError::Refused {{ .. }})` — a method of `self` is \
            called from one, `|error| self.is_refusal(error)`"
)]
pub trait FallbackIf<E, S>: Fn(&ResilienceError<E>) -> bool {}

#[diagnostic::do_not_recommend]
impl<E, F, A, O, W> FallbackIf<E, (A, W)> for F where
    F: Fn(A) -> O + Shaped<ResilienceError<E>, A, O, W>
{
}

#[diagnostic::do_not_recommend]
impl<E, F> FallbackIf<E, Refused> for F where F: Unshaped<ResilienceError<E>> {}

/// A predicate key's value, its shape read where the key is written and
/// proved `W`.
pub struct Checked<P, W> {
    value: P,
    proof: PhantomData<fn() -> W>,
}

impl<P, W> Checked<P, W> {
    fn new(value: P) -> Self {
        Self {
            value,
            proof: PhantomData,
        }
    }
}

/// A checked predicate over `V`.
pub trait Predicate<V> {
    /// Whether the predicate takes `value`.
    fn takes(&self, value: &V) -> bool;
}

impl<V, P: Fn(&V) -> bool> Predicate<V> for Checked<P, Proved> {
    fn takes(&self, value: &V) -> bool {
        (self.value)(value)
    }
}

impl<V, P> Predicate<V> for Checked<P, Refused> {
    fn takes(&self, _: &V) -> bool {
        unreachable!("a predicate refused where its key is written does not compile")
    }
}

/// How a run counts each answer of its attempts: the three predicates of its
/// method, each the default its key has when the method leaves it out.
pub struct Judging<RI, RO, RC> {
    retry_if: RI,
    retry_ok_if: RO,
    record_if: RC,
}

/// How a run of a method answering `R` counts each answer: an `Err` is sent
/// again when `retry_if` takes it, an `Ok` when `retry_ok_if` does, and an
/// `Err` is a failure in the breaker when `record_if` takes it, counting
/// there in neither direction otherwise. Each value's shape is checked here,
/// at the key that wrote it; the first argument names `R` with no turbofish,
/// so that rustc points a refusal at the value.
pub fn judging<R, RI, RO, RC, S1, S2, S3, W1, W2, W3>(
    _: PhantomData<R>,
    retry_if: RI,
    retry_ok_if: RO,
    record_if: RC,
) -> Judging<Checked<RI, W1>, Checked<RO, W2>, Checked<RC, W3>>
where
    R: ResilientAnswer,
    RI: RetryIf<R::Err, S1, W1>,
    RO: RetryOkIf<R::Ok, S2, W2>,
    RC: RecordIf<R::Err, S3, W3>,
{
    Judging {
        retry_if: Checked::new(retry_if),
        retry_ok_if: Checked::new(retry_ok_if),
        record_if: Checked::new(record_if),
    }
}

/// How a run answering `R` counts each answer when no key says otherwise —
/// a method writing none, a call run through its policy: every `Err` a
/// failure in the breaker and sent again, and no `Ok` sent again.
pub(crate) fn judging_by_default<R: ResilientAnswer>() -> impl Judge<R> {
    judging(PhantomData::<R>, every, none, every)
}

/// How a run of a method answering `R` counts each answer of its attempts.
pub trait Judge<R: ResilientAnswer> {
    /// Whether `error` is sent again.
    fn retries(&self, error: &R::Err) -> bool;

    /// Whether `answer` is sent again.
    fn retries_ok(&self, answer: &R::Ok) -> bool;

    /// Whether `error` counts as a failure in the breaker.
    fn records(&self, error: &R::Err) -> bool;
}

impl<R, RI, RO, RC> Judge<R> for Judging<RI, RO, RC>
where
    R: ResilientAnswer,
    RI: Predicate<R::Err>,
    RO: Predicate<R::Ok>,
    RC: Predicate<R::Err>,
{
    fn retries(&self, error: &R::Err) -> bool {
        self.retry_if.takes(error)
    }

    fn retries_ok(&self, answer: &R::Ok) -> bool {
        self.retry_ok_if.takes(answer)
    }

    fn records(&self, error: &R::Err) -> bool {
        self.record_if.takes(error)
    }
}

/// A fallback handed the run's error by value: read off what the value
/// takes, `A`, as [`Shaped`] reads a predicate's.
pub trait Handed<E, A, Fut> {
    /// The fallback's future, handed the run's `error`.
    fn hand(self, error: ResilienceError<E>) -> Fut;
}

impl<E, B, Fut> Handed<E, ResilienceError<E>, Fut> for B
where
    B: FnOnce(ResilienceError<E>) -> Fut,
{
    fn hand(self, error: ResilienceError<E>) -> Fut {
        self(error)
    }
}

/// What a fallback's future answers, proved `W` once it is what the method
/// answers.
pub trait Answered<T, E, W> {}

impl<T, E> Answered<T, E, Proved> for Result<T, ResilienceError<E>> {}

/// What `fallback` takes, the method's arguments already bound: the error the
/// run ended on, by value, answered by what the method answers. `A`, `Fut`
/// and `W` are the value's own shape and its proof, inferred.
#[diagnostic::on_unimplemented(
    message = "a `#[resilient]` method's `fallback` takes the error its run ended on and \
               answers what the method answers, `Result<{T}, ResilienceError<{E}>>`, and this \
               one does not",
    label = "the fallback",
    note = "the fallback is an `async fn` of `Self` taking the method's arguments, in order, \
            then the error by value, a `ResilienceError<{E}>`"
)]
pub trait Fallback<T, E, A, Fut, W> {
    /// The fallback's future, handed the run's `error`.
    fn answer(self, error: ResilienceError<E>) -> Fut;

    /// What a method's run `ended` on, or what `fallback` answers in place of
    /// its error, handed it by value: the fallback's answer is then the
    /// method's, an `Err` of its own included.
    fn in_place_of<R>(ended: Ended<R, T, E>, fallback: Self) -> FellBack<R, Fut, W>
    where
        Self: Sized,
    {
        match ended.ended {
            Ok(answered) => FellBack::ran(answered),
            Err(error) => FellBack::FallingBack {
                fallback: placed_future(fallback.answer(error)),
            },
        }
    }
}

#[diagnostic::do_not_recommend]
impl<T, E, B, A, Fut, Out, W> Fallback<T, E, A, Fut, W> for B
where
    B: FnOnce(A) -> Fut + Handed<E, A, Fut>,
    Fut: Future<Output = Out>,
    Out: Answered<T, E, W>,
{
    fn answer(self, error: ResilienceError<E>) -> Fut {
        self.hand(error)
    }
}

/// What a `#[resilient]` method's body answers, as a value nothing builds: the
/// body returns it first, under `if false`, so its own answers are checked
/// against that type.
pub fn attempt<R: ResilientAnswer>() -> R::Attempt {
    unreachable!("a `#[resilient]` body returns its attempt's type under `if false` alone")
}

/// `call` run once under `policy`, as `method`'s body, an `Err` it answers
/// counting as `judging` says. The body reads no token, so none is made.
pub fn run<R, I, J, C, F>(
    policy: &ResiliencePolicy<I>,
    method: &'static ResilientMethod,
    judging: J,
    call: C,
) -> impl Future<Output = R>
where
    R: ResilientAnswer,
    R::Err: ChainedError,
    I: Instance,
    J: Judge<R>,
    C: FnOnce() -> F,
    F: Future<Output = R::Attempt>,
{
    policy.run_once::<R, J, Uncancelled, C>(Some(method), judging, call)
}

/// [`run`], its body handed the attempt's token, which its `cancel` names.
pub fn run_cancellable<R, I, J, C, F>(
    policy: &ResiliencePolicy<I>,
    method: &'static ResilientMethod,
    judging: J,
    call: C,
) -> impl Future<Output = R>
where
    R: ResilientAnswer,
    R::Err: ChainedError,
    I: Instance,
    J: Judge<R>,
    C: FnOnce(CancellationToken) -> F,
    F: Future<Output = R::Attempt>,
{
    policy.run_once::<R, J, Cancellable, C>(Some(method), judging, call)
}

/// `call` run under `policy` as `method`'s idempotent body, attempted again
/// after a timeout or an answer `judging` says to send again. The body reads
/// no token, so none is made.
// An `async fn`, holding its closure of borrows twice (rust-lang/rust#62958), so that
// rustc's error for a future not `Send` points at the `idempotent` it is spanned at.
pub async fn run_idempotent<R, I, J, C, F>(
    policy: &ResiliencePolicy<I>,
    method: &'static ResilientMethod,
    judging: J,
    mut call: C,
) -> R
where
    R: ResilientAnswer,
    R::Err: ChainedError,
    I: Instance,
    J: Judge<R>,
    C: FnMut() -> F,
    F: Future<Output = R::Attempt>,
{
    policy
        .run_again::<R, J, Uncancelled, _>(Some(method), judging, &mut call)
        .await
}

/// [`run_idempotent`], each attempt of its body handed its own token, which
/// its `cancel` names.
// An `async fn` for the reason `run_idempotent` is one.
pub async fn run_idempotent_cancellable<R, I, J, C, F>(
    policy: &ResiliencePolicy<I>,
    method: &'static ResilientMethod,
    judging: J,
    mut call: C,
) -> R
where
    R: ResilientAnswer,
    R::Err: ChainedError,
    I: Instance,
    J: Judge<R>,
    C: FnMut(CancellationToken) -> F,
    F: Future<Output = R::Attempt>,
{
    policy
        .run_again::<R, J, Cancellable, _>(Some(method), judging, &mut call)
        .await
}

/// What `method`'s run ended on, `ran`, its filter asked: the run's own answer,
/// or the error its fallback answers in place of when `fallback_if` takes it —
/// the call's own, from its last attempt, or the policy's refusal.
///
/// Decided as it is called: the run is over, its last attempt's savepoint
/// rolled back, so what the fallback writes is its unit's. An attempt's
/// savepoint the database could not settle is the unit's own transaction
/// failing, never the dependency: no predicate is asked, and no fallback
/// answers for it.
///
/// The filter is asked and dropped here, before [`Fallback::in_place_of`] is
/// handed the fallback: what it borrows of the method's arguments, the
/// fallback then takes by value.
///
/// A filter that panics unwinds from here, and a fallback from the future
/// [`Fallback::in_place_of`] answers: no attempt is out to fail, so the run
/// stays counted as it ended and the panic files no line.
pub fn fall_back<R, T, E, I, P, S>(
    policy: &ResiliencePolicy<I>,
    method: &'static ResilientMethod,
    ran: R,
    fallback_if: P,
) -> Ended<R, T, E>
where
    R: ResilientAnswer<Ok = T, Err = E>,
    // Written of `R`, so rustc blames the answer handed in, as `run` does.
    R::Err: ChainedError,
    I: Instance,
    P: FallbackIf<E, S>,
{
    let error = match ran.failed() {
        Ok(answered) => return Ended::on(Ok(answered)),
        Err(error) => error,
    };
    if matches!(error, ResilienceError::Unsettled { .. }) || !fallback_if(&error) {
        return Ended::on(Ok(R::answered(Err(error))));
    }
    policy.mechanisms.teller.fallback(method, &error);
    Ended::on(Err(error))
}

/// What a run of a method answering `R`, a `Result<T, ResilienceError<E>>`,
/// ended on once its fallback's filter was asked: its own answer, or the error
/// its fallback answers in place of. Only [`fall_back`] builds one, so a
/// fallback is handed no error its filter refused, nor an unsettled savepoint.
// No bound on `R`: one here is a second error for an answer `fall_back` refused.
pub struct Ended<R, T, E> {
    ended: Result<R, ResilienceError<E>>,
    answers: PhantomData<fn() -> T>,
}

impl<R, T, E> Ended<R, T, E> {
    fn on(ended: Result<R, ResilienceError<E>>) -> Self {
        Self {
            ended,
            answers: PhantomData,
        }
    }
}

pin_project_lite::pin_project! {
    /// What [`Fallback::in_place_of`] answers: the run's own answer, or its
    /// fallback's, polled where a run polls an attempt's — in place in a
    /// release build, in a box of its own in a debug build — `W` being the
    /// fallback's proof.
    #[project = FellBackProj]
    pub enum FellBack<R, Fut, W> {
        /// The run's own answer.
        Ran {
            #[pin]
            ran: Ready<R>,
            proof: PhantomData<fn() -> W>,
        },
        /// The fallback's, once it answers.
        FallingBack {
            #[pin]
            fallback: PlacedFuture<Fut>,
        },
    }
}

impl<R, Fut, W> FellBack<R, Fut, W> {
    fn ran(answered: R) -> Self {
        Self::Ran {
            ran: ready(answered),
            proof: PhantomData,
        }
    }
}

impl<R, Fut> Future for FellBack<R, Fut, Proved>
where
    R: ResilientAnswer,
    Fut: Future<Output = Result<R::Ok, ResilienceError<R::Err>>>,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<R> {
        match self.project() {
            FellBackProj::Ran { ran, .. } => ran.poll(cx),
            FellBackProj::FallingBack { fallback } => {
                placed_future_mut(fallback).poll(cx).map(R::answered)
            }
        }
    }
}

impl<R, Fut> Future for FellBack<R, Fut, Refused> {
    type Output = R;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<R> {
        unreachable!("a fallback refused where its key is written does not compile")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use nest_rs_core::panic::contain;
    use nest_rs_core::{Problem, code, instance};
    use nest_rs_database::__private::with_request_executor;
    use nest_rs_database::{AtomicBlock, AtomicError, Executor, current_executor};
    use nest_rs_testing::LogCapture;
    use tokio::time::Instant;

    use super::*;
    use crate::error::{ResilienceRefusal, TIMED_OUT};
    use crate::{BreakerState, ResilienceConfig, TARGET};

    #[instance]
    struct Carrier;

    /// What the carrier answers in error: down, an answer of its own, or a
    /// wait it asks.
    #[derive(Debug, thiserror::Error)]
    enum Quote {
        #[error("the carrier is down")]
        Down,
        #[error("the carrier serves no such zone")]
        NoZone,
        #[error("the carrier asked to slow down")]
        Throttled(#[source] Problem),
    }

    type Answer = Result<u32, ResilienceError<Quote>>;

    /// Names the answer a judging is over, as the decorator names its method's.
    const ANSWER: PhantomData<Answer> = PhantomData;

    fn host() -> &'static str {
        "rates::RatesService"
    }

    const QUOTE: ResilientMethod = resilient_method(host, "quote", None, None);

    fn policy(config: ResilienceConfig) -> ResiliencePolicy<Carrier> {
        ResiliencePolicy::new(&config).expect("a valid policy")
    }

    /// Two retries, the first after 100 ms, and a breaker opening on
    /// `failures` failures of its latest `failures` attempts.
    fn guarded(failures: usize) -> ResiliencePolicy<Carrier> {
        policy(ResilienceConfig {
            timeout: Duration::from_secs(1),
            retries: Some(2),
            backoff_first: Duration::from_millis(100),
            backoff_ceiling: Duration::from_secs(5),
            breaker_window: Some(failures),
            breaker_failures: failures,
            ..ResilienceConfig::default()
        })
    }

    /// Two retries and no breaker: the subject is what a run answers.
    fn unguarded() -> ResiliencePolicy<Carrier> {
        policy(ResilienceConfig {
            retries: Some(2),
            backoff_first: Duration::from_millis(100),
            breaker_window: None,
            ..ResilienceConfig::default()
        })
    }

    fn is_down(error: &Quote) -> bool {
        matches!(error, Quote::Down)
    }

    /// The idempotent run of a call answering `answers` of its attempt,
    /// counted from one in `attempts`, each answer counted as `judging` says.
    async fn quoted(
        policy: &ResiliencePolicy<Carrier>,
        attempts: &AtomicU32,
        judging: impl Judge<Answer>,
        answers: impl Fn(u32) -> Result<u32, Quote>,
    ) -> Answer {
        run_idempotent::<Answer, _, _, _, _>(policy, &QUOTE, judging, || {
            let attempt = attempts.fetch_add(1, Ordering::Relaxed) + 1;
            ready(answers(attempt))
        })
        .await
    }

    /// The retry axis is `retry_if`'s alone: an `Err` `record_if` refuses is
    /// sent again when it takes it, each retry's line telling its chain, and
    /// none of them is an outcome of the breaker's window.
    #[tokio::test(start_paused = true)]
    async fn an_err_retry_if_takes_is_sent_again_though_record_if_counts_it_for_no_failure() {
        let policy = guarded(2);
        let attempts = AtomicU32::new(0);
        let lines = LogCapture::install();
        for _ in 0..2 {
            let ran = quoted(
                &policy,
                &attempts,
                judging(ANSWER, every, none, none),
                |_| Err(Quote::NoZone),
            )
            .await;
            assert!(
                matches!(ran, Err(ResilienceError::Failed(Quote::NoZone))),
                "{ran:?}"
            );
        }
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            6,
            "each run sends its call again up to its two retries"
        );
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Closed),
            "six answers, none a failure, in a breaker opening on two"
        );
        let counted = policy.breaker_stats().expect("the policy has a breaker");
        assert_eq!(
            (counted.outcomes, counted.failures),
            (0, 0),
            "and none a success: the window holds nothing of them"
        );
        let retried = lines.find(TARGET, crate::teller::RETRYING_FAILED);
        assert_eq!(retried.len(), 4, "{retried:#?}");
        assert!(
            retried.iter().all(|line| {
                line.field("error").as_deref() == Some("the carrier serves no such zone")
            }),
            "each retry tells what the attempt before it answered: {retried:#?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_err_record_if_takes_is_a_failure_though_retry_if_sends_it_no_more() {
        let policy = guarded(2);
        let attempts = AtomicU32::new(0);
        for _ in 0..2 {
            let ran = quoted(
                &policy,
                &attempts,
                judging(ANSWER, none, none, is_down),
                |_| Err(Quote::Down),
            )
            .await;
            assert!(
                matches!(ran, Err(ResilienceError::Failed(Quote::Down))),
                "{ran:?}"
            );
        }
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            2,
            "each run ends on its first attempt, whatever retries its policy has"
        );
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "two failures open the breaker: {:?}",
            policy.breaker_state()
        );
    }

    /// One run of a call sent once answering `answer`, an `Err` a failure when
    /// the carrier is down.
    async fn quoted_once(policy: &ResiliencePolicy<Carrier>, answer: Result<u32, Quote>) -> Answer {
        let judging = judging(ANSWER, every, none, is_down);
        run::<Answer, _, _, _, _>(policy, &QUOTE, judging, || ready(answer)).await
    }

    /// An `Err` `record_if` refuses counts in neither direction. Closed, the
    /// breaker holds nothing of it: the failure before it stays in a window
    /// more such answers than it holds came after, and nothing is filed. As a
    /// probe's answer it hands the turn on, the series as it was.
    #[tokio::test(start_paused = true)]
    async fn an_err_record_if_refuses_counts_in_neither_direction_and_hands_a_probes_turn_on() {
        const MOVED: &str = crate::teller::MOVED;
        let policy = policy(ResilienceConfig {
            retries: None,
            breaker_window: Some(3),
            breaker_failures: 2,
            breaker_open: Duration::from_secs(30),
            breaker_probes: 2,
            ..ResilienceConfig::default()
        });
        let counted = |policy: &ResiliencePolicy<Carrier>| {
            let stats = policy.breaker_stats().expect("the policy has a breaker");
            (stats.outcomes, stats.failures)
        };

        let lines = LogCapture::install();
        let failed = quoted_once(&policy, Err(Quote::Down)).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Quote::Down))));
        for _ in 0..4 {
            let answered = quoted_once(&policy, Err(Quote::NoZone)).await;
            assert!(
                matches!(answered, Err(ResilienceError::Failed(Quote::NoZone))),
                "{answered:?}"
            );
            assert_eq!(counted(&policy), (1, 1), "the one failure, and no other");
        }
        assert!(lines.find(TARGET, MOVED).is_empty(), "nothing moved");
        let failed = quoted_once(&policy, Err(Quote::Down)).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Quote::Down))));
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "the second failure of a window that forgot none: {:?}",
            policy.breaker_state()
        );
        drop(lines);

        tokio::time::advance(Duration::from_secs(30)).await;
        for (answer, then) in [
            (Err(Quote::NoZone), BreakerState::HalfOpen),
            (Err(Quote::NoZone), BreakerState::HalfOpen),
            (Ok(7), BreakerState::HalfOpen),
            (Err(Quote::NoZone), BreakerState::HalfOpen),
            (Ok(7), BreakerState::Closed),
        ] {
            let probed = quoted_once(&policy, answer).await;
            assert!(
                matches!(probed, Ok(7) | Err(ResilienceError::Failed(Quote::NoZone))),
                "each is sent as the probe, its answer its caller's: {probed:?}"
            );
            assert_eq!(
                policy.breaker_state(),
                Some(then),
                "two `Ok`s close it, an `Err` it refuses neither advancing the series nor \
                 starting it over: after {probed:?}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_ok_retry_ok_if_takes_is_sent_again_and_the_last_answers_once_the_retries_are_spent()
    {
        const RETRIED: &str = crate::teller::RETRYING_ANSWERED;
        let policy = guarded(2);
        let pending = |quote: &u32| *quote < 10;

        let lines = LogCapture::install();
        let attempts = AtomicU32::new(0);
        let spent = quoted(
            &policy,
            &attempts,
            judging(ANSWER, every, pending, every),
            Ok,
        )
        .await;
        assert!(
            matches!(spent, Ok(3)),
            "the third attempt's `Ok`, the two retries spent: {spent:?}"
        );
        assert_eq!(attempts.load(Ordering::Relaxed), 3);
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Closed),
            "each `Ok` sent again counts as a success"
        );
        let retried = lines.find(TARGET, RETRIED);
        assert_eq!(retried.len(), 2, "{retried:#?}");
        assert!(
            retried.iter().all(|line| line.field("error").is_none()),
            "an `Ok` sent again has no error to tell: {retried:#?}"
        );
        lines.expect_none(TARGET, crate::teller::RETRYING_FAILED);
        drop(lines);

        let attempts = AtomicU32::new(0);
        let passed = quoted(
            &policy,
            &attempts,
            judging(ANSWER, every, |quote| *quote < 2, every),
            Ok,
        )
        .await;
        assert!(
            matches!(passed, Ok(2)),
            "the first `Ok` the predicate lets pass ends the run: {passed:?}"
        );
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
    }

    /// The wait is the dependency's ask whichever predicate sent the call
    /// again: here `retry_if` alone, of an `Err` no failure, stretched by a
    /// fifth at most.
    #[tokio::test(start_paused = true)]
    async fn the_wait_an_err_asks_is_honoured_for_one_sent_again_and_counted_for_no_failure() {
        let policy = guarded(1);
        let attempts = AtomicU32::new(0);
        let started = Instant::now();
        let ran = quoted(
            &policy,
            &attempts,
            judging(ANSWER, every, none, none),
            |attempt| match attempt {
                1 => Err(Quote::Throttled(
                    Problem::new(429, code::RATE_LIMITED).with_retry_after(3),
                )),
                _ => Ok(7),
            },
        )
        .await;
        assert!(matches!(ran, Ok(7)), "{ran:?}");
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_secs(3) && waited <= Duration::from_millis(3_600),
            "{waited:?}"
        );
    }

    /// A run holds its judging in place: the defaults weigh nothing, and a
    /// predicate what it captures.
    #[test]
    fn a_key_left_out_weighs_nothing_in_its_run() {
        assert_eq!(size_of_val(&judging(ANSWER, every, none, every)), 0);
        assert_eq!(size_of_val(&judging(ANSWER, is_down, none, is_down)), 0);
        let floor = 3_u32;
        let above = |quote: &u32| *quote > floor;
        assert_eq!(
            size_of_val(&judging(ANSWER, every, above, every)),
            size_of_val(&above)
        );
    }

    /// A timeout is no `Err` of the call's: sent again and counted, with no
    /// predicate asked.
    #[tokio::test(start_paused = true)]
    async fn a_timeout_is_a_failure_sent_again_whatever_the_predicates_and_none_reads_it() {
        fn unread<V>(_: &V) -> bool {
            unreachable!("no predicate reads a timeout")
        }

        let policy = guarded(3);
        let attempts = AtomicU32::new(0);
        let unread = judging(ANSWER, unread, unread, unread);
        let ran = run_idempotent::<Answer, _, _, _, _>(&policy, &QUOTE, unread, || {
            attempts.fetch_add(1, Ordering::Relaxed);
            std::future::pending::<Result<u32, Quote>>()
        })
        .await;
        assert!(
            matches!(
                ran,
                Err(ResilienceError::Refused {
                    refusal: ResilienceRefusal::TimedOut { .. },
                    ..
                })
            ),
            "{ran:?}"
        );
        assert_eq!(attempts.load(Ordering::Relaxed), 3);
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "three timeouts, three failures: {:?}",
            policy.breaker_state()
        );
    }

    /// Which predicate of a run panics.
    #[derive(Clone, Copy, Debug)]
    enum Site {
        RetryIf,
        RetryOkIf,
        RecordIf,
        RecordIfOnce,
    }

    fn panics<V>(_: &V) -> bool {
        panic!("the predicate panicked")
    }

    /// Whether a run whose predicate at `site` panics unwinds; with no panic,
    /// each would be an answer, never a failure.
    async fn unwinds(policy: &ResiliencePolicy<Carrier>, site: Site) -> bool {
        let down = || ready(Err::<u32, _>(Quote::Down));
        let up = || ready(Ok::<_, Quote>(7));
        let unwound = match site {
            Site::RetryIf => {
                let judging = judging(ANSWER, panics, none, none);
                contain(run_idempotent::<Answer, _, _, _, _>(
                    policy, &QUOTE, judging, down,
                ))
                .await
            }
            Site::RetryOkIf => {
                let judging = judging(ANSWER, every, panics, every);
                contain(run_idempotent::<Answer, _, _, _, _>(
                    policy, &QUOTE, judging, up,
                ))
                .await
            }
            Site::RecordIf => {
                let judging = judging(ANSWER, none, none, panics);
                contain(run_idempotent::<Answer, _, _, _, _>(
                    policy, &QUOTE, judging, down,
                ))
                .await
            }
            Site::RecordIfOnce => {
                let judging = judging(ANSWER, every, none, panics);
                contain(run::<Answer, _, _, _, _>(policy, &QUOTE, judging, down)).await
            }
        };
        unwound.is_err()
    }

    /// A predicate judging an attempt's answer is part of the call's run: its
    /// panic counts as one failure, as the call's own does, and unwinds on.
    #[tokio::test(start_paused = true)]
    async fn a_predicate_that_panics_counts_as_a_failure_and_unwinds_on() {
        for site in [
            Site::RetryIf,
            Site::RetryOkIf,
            Site::RecordIf,
            Site::RecordIfOnce,
        ] {
            let policy = guarded(2);
            assert!(unwinds(&policy, site).await, "{site:?}: the panic unwinds");
            assert_eq!(
                policy.breaker_state(),
                Some(BreakerState::Closed),
                "{site:?}: one panic is one failure, of the two the breaker opens on"
            );
            assert!(unwinds(&policy, site).await, "{site:?}: the panic unwinds");
            assert!(
                matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
                "{site:?}: a second panic is the second failure: {:?}",
                policy.breaker_state()
            );
        }
    }

    /// `refusal`, as the default instance's policy answers it.
    fn default_refusal(refusal: ResilienceRefusal) -> ResilienceError<Quote> {
        ResilienceError::refused(refusal, "")
    }

    /// Each error a run ends on but an unsettled savepoint, and what it says.
    fn errors() -> [(ResilienceError<Quote>, &'static str); 5] {
        [
            (ResilienceError::Failed(Quote::Down), "the call failed"),
            (
                default_refusal(ResilienceRefusal::TimedOut {
                    timeout: Duration::from_secs(1),
                }),
                TIMED_OUT,
            ),
            (
                default_refusal(ResilienceRefusal::Open {
                    retry_after: Duration::from_secs(30),
                }),
                "the circuit breaker is open",
            ),
            (
                default_refusal(ResilienceRefusal::Full),
                "the bulkhead has no free slot",
            ),
            (
                default_refusal(ResilienceRefusal::Limited {
                    retry_after: Duration::from_secs(2),
                }),
                "the rate limit is reached",
            ),
        ]
    }

    #[tokio::test]
    async fn a_fallback_is_handed_the_calls_own_error_and_each_refusal_of_the_policy_by_value() {
        let policy = unguarded();
        for (error, says) in errors() {
            let handed = Mutex::new(None);
            let answered = Fallback::in_place_of(
                fall_back::<Answer, _, _, _, _, _>(&policy, &QUOTE, Err(error), every),
                |error: ResilienceError<Quote>| async {
                    *handed.lock().expect("the fallback's own") = Some(error);
                    Ok(7)
                },
            )
            .await;
            assert!(matches!(answered, Ok(7)), "{says}: {answered:?}");
            let handed = handed.into_inner().expect("the fallback's own");
            assert_eq!(handed.map(|error| error.to_string()).as_deref(), Some(says));
        }
    }

    #[tokio::test]
    async fn an_error_fallback_if_refuses_is_the_methods_and_its_fallback_is_not_called() {
        let policy = unguarded();
        let called = AtomicU32::new(0);
        let stale = |_: ResilienceError<Quote>| async {
            called.fetch_add(1, Ordering::Relaxed);
            Ok(7)
        };
        let open = |error: &ResilienceError<Quote>| {
            matches!(
                error,
                ResilienceError::Refused {
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                }
            )
        };

        let refused = Fallback::in_place_of(
            fall_back::<Answer, _, _, _, _, _>(
                &policy,
                &QUOTE,
                Err(ResilienceError::Failed(Quote::Down)),
                open,
            ),
            stale,
        )
        .await;
        assert!(
            matches!(refused, Err(ResilienceError::Failed(Quote::Down))),
            "{refused:?}"
        );
        assert_eq!(called.load(Ordering::Relaxed), 0);

        let taken = Fallback::in_place_of(
            fall_back::<Answer, _, _, _, _, _>(
                &policy,
                &QUOTE,
                Err(default_refusal(ResilienceRefusal::Open {
                    retry_after: Duration::from_secs(30),
                })),
                open,
            ),
            stale,
        )
        .await;
        assert!(matches!(taken, Ok(7)), "{taken:?}");
        assert_eq!(called.load(Ordering::Relaxed), 1);
    }

    /// The filter is asked and dropped before the fallback is handed over: what
    /// it borrowed, the fallback owns, whichever of the two answers.
    #[tokio::test]
    async fn a_filter_reads_what_its_fallback_then_takes_by_value() {
        let policy = unguarded();
        for (zone, answers) in [("eu", 2), ("us", 0)] {
            let zone = zone.to_owned();
            let down = Err(ResilienceError::Failed(Quote::Down));
            let served = |_: &ResilienceError<Quote>| zone == "eu";
            let answered = Fallback::in_place_of(
                fall_back::<Answer, _, _, _, _, _>(&policy, &QUOTE, down, served),
                move |_: ResilienceError<Quote>| async move { Ok(zone.len() as u32) },
            )
            .await;
            assert_eq!(answered.unwrap_or(0), answers);
        }
    }

    #[tokio::test]
    async fn a_fallbacks_own_err_is_the_methods_answer_and_an_ok_run_asks_nothing() {
        let policy = unguarded();
        let own = Fallback::in_place_of(
            fall_back::<Answer, _, _, _, _, _>(
                &policy,
                &QUOTE,
                Err(ResilienceError::Failed(Quote::Down)),
                every,
            ),
            |_: ResilienceError<Quote>| async { Err(ResilienceError::Failed(Quote::NoZone)) },
        )
        .await;
        assert!(
            matches!(own, Err(ResilienceError::Failed(Quote::NoZone))),
            "{own:?}"
        );

        let asked = AtomicU32::new(0);
        let answered = Fallback::in_place_of(
            fall_back::<Answer, _, _, _, _, _>(
                &policy,
                &QUOTE,
                Ok(3),
                |_: &ResilienceError<Quote>| {
                    asked.fetch_add(1, Ordering::Relaxed);
                    true
                },
            ),
            |_: ResilienceError<Quote>| async {
                asked.fetch_add(1, Ordering::Relaxed);
                Ok(7)
            },
        )
        .await;
        assert!(matches!(answered, Ok(3)), "{answered:?}");
        assert_eq!(asked.load(Ordering::Relaxed), 0);
    }

    /// The future holds the run's answer or the fallback's future, never
    /// both, and neither twice: in a release build the fallback's in place,
    /// in a debug build its box, whatever it holds.
    #[tokio::test]
    async fn a_fallbacks_future_weighs_its_runs_answer_or_its_fallbacks_future_once() {
        const HELD: usize = 4 * 1024;
        type Large = Result<[u8; HELD], ResilienceError<Quote>>;

        let policy = unguarded();
        let stale = |_: ResilienceError<Quote>| async {
            let buffer = [7_u8; HELD];
            tokio::task::yield_now().await;
            Ok(buffer)
        };
        let made = size_of_val(&stale(ResilienceError::Failed(Quote::Down)));
        let down = Err(ResilienceError::Failed(Quote::Down));
        let falling = Fallback::in_place_of(
            fall_back::<Large, _, _, _, _, _>(&policy, &QUOTE, down, every),
            stale,
        );
        let weighs = size_of_val(&falling);
        let held = if cfg!(debug_assertions) {
            size_of::<usize>()
        } else {
            made
        };
        assert!(
            weighs <= held.max(size_of::<Large>()) + size_of::<usize>(),
            "{weighs} B, for a fallback's future of {made} B and an answer of {} B",
            size_of::<Large>()
        );
        assert!(matches!(falling.await, Ok(buffer) if buffer == [7; HELD]));

        let laden = |_: ResilienceError<Quote>| async {
            let buffer = [7_u8; 4 * HELD];
            tokio::task::yield_now().await;
            Ok(buffer.len() as u32)
        };
        let made = size_of_val(&laden(ResilienceError::Failed(Quote::Down)));
        let down = Err(ResilienceError::Failed(Quote::Down));
        let falling = Fallback::in_place_of(
            fall_back::<Answer, _, _, _, _, _>(&policy, &QUOTE, down, every),
            laden,
        );
        let weighs = size_of_val(&falling);
        if cfg!(debug_assertions) {
            assert!(
                weighs <= size_of::<Answer>() + 2 * size_of::<usize>(),
                "{weighs} B in a debug build, for a fallback's future of {made} B"
            );
        } else {
            assert!(
                made <= weighs && weighs <= made + size_of::<usize>(),
                "{weighs} B in a release build, for a fallback's future of {made} B"
            );
        }
        assert!(matches!(falling.await, Ok(answered) if answered as usize == 4 * HELD));
    }

    /// Through the two calls a decorated method makes: the run, then its
    /// fallback over what the run answered.
    #[tokio::test(start_paused = true)]
    async fn a_run_ending_on_its_last_attempts_error_or_refused_unsent_is_answered_by_its_fallback()
    {
        let stale = |error: ResilienceError<Quote>| async move {
            match error {
                ResilienceError::Failed(Quote::Down) => Ok(1),
                ResilienceError::Refused {
                    refusal: ResilienceRefusal::Open { .. },
                    ..
                } => Ok(2),
                other => Err(other),
            }
        };

        let retried = unguarded();
        let attempts = AtomicU32::new(0);
        let ran = quoted(
            &retried,
            &attempts,
            judging(ANSWER, every, none, every),
            |_| Err(Quote::Down),
        )
        .await;
        let answered = Fallback::in_place_of(
            fall_back::<Answer, _, _, _, _, _>(&retried, &QUOTE, ran, every),
            stale,
        )
        .await;
        assert!(matches!(answered, Ok(1)), "{answered:?}");
        assert_eq!(
            attempts.load(Ordering::Relaxed),
            3,
            "the fallback answers once the retries are spent"
        );

        let opening = guarded(1);
        let ran = quoted_once(&opening, Err(Quote::Down)).await;
        let answered = Fallback::in_place_of(
            fall_back::<Answer, _, _, _, _, _>(&opening, &QUOTE, ran, every),
            stale,
        )
        .await;
        assert!(matches!(answered, Ok(1)), "{answered:?}");
        let ran = quoted_once(&opening, Ok(9)).await;
        let answered = Fallback::in_place_of(
            fall_back::<Answer, _, _, _, _, _>(&opening, &QUOTE, ran, every),
            stale,
        )
        .await;
        assert!(
            matches!(answered, Ok(2)),
            "refused unsent by the breaker its failure opened: {answered:?}"
        );
    }

    /// The line names the method as its run's lines do and tells the error's
    /// whole chain; a run its fallback does not answer for files none.
    #[tokio::test]
    async fn a_fallback_answering_files_a_debug_line_with_the_errors_chain() {
        const FELL_BACK: &str = crate::teller::FALLBACK_ANSWERS;
        let policy = unguarded();
        let stale = |_: ResilienceError<Quote>| async { Ok(7) };
        let throttled =
            || ResilienceError::Failed(Quote::Throttled(Problem::new(429, code::RATE_LIMITED)));

        let lines = LogCapture::install();
        let kept = Fallback::in_place_of(
            fall_back::<Answer, _, _, _, _, _>(&policy, &QUOTE, Err(throttled()), none),
            stale,
        )
        .await;
        assert!(kept.is_err(), "{kept:?}");
        let answered = Fallback::in_place_of(
            fall_back::<Answer, _, _, _, _, _>(&policy, &QUOTE, Ok(3), every),
            stale,
        )
        .await;
        assert!(matches!(answered, Ok(3)), "{answered:?}");
        lines.expect_none(TARGET, FELL_BACK);

        let fell_back = Fallback::in_place_of(
            fall_back::<Answer, _, _, _, _, _>(&policy, &QUOTE, Err(throttled()), every),
            stale,
        )
        .await;
        assert!(matches!(fell_back, Ok(7)), "{fell_back:?}");
        let line = lines.expect_one(TARGET, FELL_BACK);
        assert_eq!(line.level, "debug", "{line:?}");
        for (field, says) in [
            ("instance", "carrier"),
            ("host", "RatesService"),
            ("method", "quote"),
            (
                "error",
                "the call failed: the carrier asked to slow down: 429 RATE_LIMITED",
            ),
        ] {
            assert_eq!(line.field(field).as_deref(), Some(says), "{line:?}");
        }
    }

    #[cfg(feature = "opentelemetry")]
    #[tokio::test]
    async fn each_run_its_fallback_answers_for_is_counted() {
        use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
        use opentelemetry_sdk::metrics::{
            InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        };

        use crate::counters::{FALLBACKS, INSTANCE_ATTRIBUTE};

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());

        let policy = unguarded();
        let stale = |_: ResilienceError<Quote>| async { Ok(7) };
        let down = || Err(ResilienceError::Failed(Quote::Down));
        for fallback_if in [every, every, none] {
            let _answered: Answer = Fallback::in_place_of(
                fall_back::<Answer, _, _, _, _, _>(&policy, &QUOTE, down(), fallback_if),
                stale,
            )
            .await;
        }
        let _answered: Answer = Fallback::in_place_of(
            fall_back::<Answer, _, _, _, _, _>(&policy, &QUOTE, Ok(3), every),
            stale,
        )
        .await;

        provider.force_flush().expect("the export");
        let exported = exporter
            .get_finished_metrics()
            .expect("the exported metrics");
        let counted: Vec<(String, u64)> = exported
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .filter(|metric| metric.name() == FALLBACKS)
            .flat_map(|metric| {
                let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                    panic!("a counter: {metric:?}");
                };
                sum.data_points()
                    .map(|point| {
                        let instance = point
                            .attributes()
                            .find(|kv| kv.key.as_str() == INSTANCE_ATTRIBUTE)
                            .map(|kv| kv.value.to_string())
                            .unwrap_or_default();
                        (instance, point.value())
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(
            counted,
            [("carrier".to_owned(), 2)],
            "the two runs a fallback answered for, never one it refused nor one that answered"
        );
    }

    /// What a unit holding a transaction, its savepoints, a call and a
    /// fallback saw, in order.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Saw {
        Opened,
        /// A call polled, inside a savepoint or not.
        Called {
            inside: bool,
        },
        Kept,
        RolledBack,
        /// A fallback called, inside a savepoint or not.
        FellBack {
            inside: bool,
        },
    }

    #[derive(Default)]
    struct Seen(Mutex<Vec<Saw>>);

    impl Seen {
        fn saw(&self, saw: Saw) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(saw);
        }

        fn all(&self) -> Vec<Saw> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    /// A unit of work holding a transaction: its driver offers a savepoint,
    /// settling as `settles` answers.
    struct Unit {
        seen: Arc<Seen>,
        settles: fn() -> Result<(), AtomicError>,
    }

    impl Unit {
        fn settling(settles: fn() -> Result<(), AtomicError>) -> (Arc<dyn Executor>, Arc<Seen>) {
            let seen = Arc::new(Seen::default());
            let unit = Arc::new(Self {
                seen: Arc::clone(&seen),
                settles,
            });
            (unit, seen)
        }
    }

    impl Executor for Unit {
        fn holds(&self) -> nest_rs_database::Holds<'_> {
            nest_rs_database::Holds::Transaction(self)
        }
    }

    impl nest_rs_database::HeldTransaction for Unit {
        fn after_commit(&self, _work: nest_rs_database::Deferred) {
            unreachable!("no test holds work on this unit")
        }

        fn savepoint(&self) -> Arc<dyn nest_rs_database::AtomicBlock> {
            Block::open(&self.seen, self.settles)
        }

        fn outbox(&self) -> Option<&dyn nest_rs_database::Outbox> {
            None
        }
    }

    struct Block {
        seen: Arc<Seen>,
        settles: fn() -> Result<(), AtomicError>,
    }

    impl Block {
        fn open(
            seen: &Arc<Seen>,
            settles: fn() -> Result<(), AtomicError>,
        ) -> Arc<dyn AtomicBlock> {
            seen.saw(Saw::Opened);
            Arc::new(Self {
                seen: Arc::clone(seen),
                settles,
            })
        }
    }

    impl Executor for Block {
        fn holds(&self) -> nest_rs_database::Holds<'_> {
            nest_rs_database::Holds::Transaction(self)
        }
    }

    impl nest_rs_database::HeldTransaction for Block {
        fn after_commit(&self, _work: nest_rs_database::Deferred) {
            unreachable!("no test holds work on this savepoint")
        }

        fn savepoint(&self) -> Arc<dyn nest_rs_database::AtomicBlock> {
            Self::open(&self.seen, self.settles)
        }

        fn outbox(&self) -> Option<&dyn nest_rs_database::Outbox> {
            None
        }
    }

    impl AtomicBlock for Block {
        fn settle(
            self: Arc<Self>,
            ok: bool,
        ) -> Pin<Box<dyn Future<Output = Result<(), AtomicError>> + Send>> {
            Box::pin(async move {
                self.seen.saw(if ok { Saw::Kept } else { Saw::RolledBack });
                (self.settles)()
            })
        }
    }

    fn settles() -> Result<(), AtomicError> {
        Ok(())
    }

    fn refuses() -> Result<(), AtomicError> {
        Err(AtomicError::Database("ROLLBACK TO refused".into()))
    }

    /// Whether the ambient executor is an attempt's savepoint.
    fn inside() -> bool {
        current_executor().is_some_and(|ambient| ambient.holds().downcast_ref::<Block>().is_some())
    }

    /// The run, sent once, of a method declaring a fallback, in a unit
    /// holding a transaction: its call answers `answer`, and its fallback,
    /// answering `7`, is asked for the errors `fallback_if` takes.
    async fn falling_back(
        unit: Arc<dyn Executor>,
        seen: &Seen,
        answer: Result<u32, Quote>,
        fallback_if: fn(&ResilienceError<Quote>) -> bool,
    ) -> Answer {
        let policy = unguarded();
        let method = const { &QUOTE.with_fallback() };
        with_request_executor(unit, async {
            let unrecorded = judging(ANSWER, every, none, none);
            let ran = run::<Answer, _, _, _, _>(&policy, method, unrecorded, || {
                seen.saw(Saw::Called { inside: inside() });
                ready(answer)
            })
            .await;
            Fallback::in_place_of(
                fall_back::<Answer, _, _, _, _, _>(&policy, method, ran, fallback_if),
                |_| async {
                    seen.saw(Saw::FellBack { inside: inside() });
                    Ok::<_, ResilienceError<Quote>>(7)
                },
            )
            .await
        })
        .await
    }

    /// One rule under a fallback: a failed attempt leaves no write, whether
    /// the fallback then answers, on the unit's own transaction, or its
    /// filter hands the error back — an `Err` counted for no failure included.
    #[tokio::test]
    async fn under_a_fallback_a_failed_attempt_is_rolled_back_before_the_fallback_runs_or_is_refused()
     {
        let (unit, seen) = Unit::settling(settles);
        let fell_back = falling_back(unit, &seen, Err(Quote::Down), every).await;
        assert!(matches!(fell_back, Ok(7)), "{fell_back:?}");
        assert_eq!(
            seen.all(),
            [
                Saw::Opened,
                Saw::Called { inside: true },
                Saw::RolledBack,
                Saw::FellBack { inside: false },
            ]
        );

        let (unit, seen) = Unit::settling(settles);
        let refused = falling_back(unit, &seen, Err(Quote::Down), none).await;
        assert!(
            matches!(refused, Err(ResilienceError::Failed(Quote::Down))),
            "{refused:?}"
        );
        assert_eq!(
            seen.all(),
            [Saw::Opened, Saw::Called { inside: true }, Saw::RolledBack]
        );

        let (unit, seen) = Unit::settling(settles);
        let answered = falling_back(unit, &seen, Ok(3), every).await;
        assert!(matches!(answered, Ok(3)), "{answered:?}");
        assert_eq!(
            seen.all(),
            [Saw::Opened, Saw::Called { inside: true }, Saw::Kept]
        );
    }

    /// A savepoint the database could not settle is the unit's own
    /// transaction failing: neither the filter nor the fallback is asked.
    #[tokio::test]
    async fn a_run_ending_on_an_unsettled_savepoint_is_never_handed_to_its_fallback() {
        fn unasked(_: &ResilienceError<Quote>) -> bool {
            unreachable!("no filter reads an unsettled savepoint")
        }

        let (unit, seen) = Unit::settling(refuses);
        let unsettled = falling_back(unit, &seen, Err(Quote::Down), unasked).await;
        assert!(
            matches!(unsettled, Err(ResilienceError::Unsettled { .. })),
            "{unsettled:?}"
        );
        assert_eq!(
            seen.all(),
            [Saw::Opened, Saw::Called { inside: true }, Saw::RolledBack]
        );
    }

    async fn panicking(_: ResilienceError<Quote>) -> Answer {
        panic!("the fallback panicked")
    }

    /// `fallback_if` and the fallback are asked once the run is over, outside
    /// every attempt: a panic in either unwinds on and fails none. The run
    /// stays counted as it ended — its `Err` once in the breaker's window, a
    /// refusal not at all — its slot free and its attempt rolled back, and
    /// the one line is the fallback's, filed before it was called.
    #[tokio::test(start_paused = true)]
    async fn a_filter_or_a_fallback_that_panics_unwinds_from_a_run_already_counted() {
        const FELL_BACK: &str = crate::teller::FALLBACK_ANSWERS;
        #[derive(Clone, Copy, Debug)]
        enum Panics {
            Filter,
            Fallback,
        }

        let method = const { &QUOTE.with_fallback() };
        for (panics_in, refused, counted, saw) in [
            (
                Panics::Filter,
                false,
                (1, 1),
                vec![Saw::Opened, Saw::Called { inside: true }, Saw::RolledBack],
            ),
            (Panics::Filter, true, (0, 0), vec![]),
            (
                Panics::Fallback,
                false,
                (1, 1),
                vec![Saw::Opened, Saw::Called { inside: true }, Saw::RolledBack],
            ),
            (Panics::Fallback, true, (0, 0), vec![]),
        ] {
            let policy = policy(ResilienceConfig {
                retries: None,
                breaker_window: Some(3),
                breaker_failures: 2,
                concurrency: Some(1),
                ..ResilienceConfig::default()
            });
            if refused {
                policy
                    .open_breaker(Duration::from_secs(30))
                    .expect("the policy has a breaker");
            }
            let read = || {
                let stats = policy.breaker_stats().expect("the policy has a breaker");
                (
                    (stats.outcomes, stats.failures),
                    policy.breaker_state(),
                    policy.concurrency_running(),
                )
            };
            let (unit, seen) = Unit::settling(settles);
            let lines = LogCapture::install();
            let filed = || {
                let own = lines.events();
                own.iter().filter(|line| line.target == TARGET).count()
            };
            let (unwound, before, filed_before) = with_request_executor(unit, async {
                let ran = run::<Answer, _, _, _, _>(
                    &policy,
                    method,
                    judging(ANSWER, every, none, every),
                    || {
                        seen.saw(Saw::Called { inside: inside() });
                        ready(Err(Quote::Down))
                    },
                )
                .await;
                assert_eq!(
                    matches!(
                        ran,
                        Err(ResilienceError::Refused {
                            refusal: ResilienceRefusal::Open { .. },
                            ..
                        })
                    ),
                    refused,
                    "{panics_in:?}: {ran:?}"
                );
                let before = (read(), filed());
                let unwound = match panics_in {
                    Panics::Filter => contain(async {
                        Fallback::in_place_of(
                            fall_back::<Answer, _, _, _, _, _>(&policy, method, ran, panics),
                            panicking,
                        )
                        .await
                    })
                    .await
                    .is_err(),
                    Panics::Fallback => contain(Fallback::in_place_of(
                        fall_back::<Answer, _, _, _, _, _>(&policy, method, ran, every),
                        panicking,
                    ))
                    .await
                    .is_err(),
                };
                (unwound, before.0, before.1)
            })
            .await;
            let case = format!("a panic in the {panics_in:?}, the run refused unsent: {refused}");
            assert!(unwound, "{case}");
            assert_eq!(before.0, counted, "{case}: the run, counted as it ended");
            assert_eq!(read(), before, "{case}: the panic counts nothing");
            assert_eq!(before.2, Some(0), "{case}: the run gave its slot back");
            assert_eq!(seen.all(), saw, "{case}");
            let fell_back = lines.find(TARGET, FELL_BACK).len();
            assert_eq!(
                (filed() - filed_before, fell_back),
                match panics_in {
                    Panics::Filter => (0, 0),
                    Panics::Fallback => (1, 1),
                },
                "{case}: no line but the one saying a fallback answers"
            );
        }
    }

    /// An `Ok` sent again is rolled back as a failed attempt is, the one the
    /// run answers kept; and a predicate's panic rolls its attempt back
    /// before it unwinds on.
    #[tokio::test(start_paused = true)]
    async fn an_ok_sent_again_and_an_attempt_whose_predicate_panics_are_rolled_back() {
        let policy = unguarded();

        let (unit, seen) = Unit::settling(settles);
        let attempts = AtomicU32::new(0);
        let ran = with_request_executor(
            unit,
            quoted(
                &policy,
                &attempts,
                judging(ANSWER, every, |quote| *quote < 2, every),
                |attempt| {
                    seen.saw(Saw::Called { inside: inside() });
                    Ok(attempt)
                },
            ),
        )
        .await;
        assert!(matches!(ran, Ok(2)), "{ran:?}");
        assert_eq!(
            seen.all(),
            [
                Saw::Opened,
                Saw::Called { inside: true },
                Saw::RolledBack,
                Saw::Opened,
                Saw::Called { inside: true },
                Saw::Kept,
            ]
        );

        let (unit, seen) = Unit::settling(settles);
        let attempts = AtomicU32::new(0);
        let unwound = contain(with_request_executor(
            unit,
            quoted(
                &policy,
                &attempts,
                judging(ANSWER, every, panics, every),
                |attempt| {
                    seen.saw(Saw::Called { inside: inside() });
                    Ok(attempt)
                },
            ),
        ))
        .await;
        assert!(unwound.is_err(), "the predicate's panic unwinds on");
        assert_eq!(
            seen.all(),
            [Saw::Opened, Saw::Called { inside: true }, Saw::RolledBack]
        );
    }
}
