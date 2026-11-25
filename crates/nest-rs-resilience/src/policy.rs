//! [`ResiliencePolicy`] — one named policy around the calls to one dependency:
//! a timeout per attempt, retries for an idempotent call, a circuit breaker, a
//! bulkhead and an outbound rate limit, every one of them shared by every
//! caller in the process.

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::num::NonZeroU32;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use nest_rs_config::{ConfigError, Namespaced, instance_namespace};
use nest_rs_core::__private::WithinHeld;
use nest_rs_core::{Backoff, Instance, Problem, Unanswered, Unnamed, process_seed};
use nest_rs_database::AtomicError;
use tokio::time::{Instant, Sleep};
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::TARGET;
#[cfg(feature = "reqwest")]
use crate::breaker::HeldAttempt;
use crate::breaker::{Breaker, BreakerState, BreakerStats, Cut, Pass, Sent};
#[cfg(feature = "reqwest")]
use crate::bulkhead::HeldSlot;
use crate::bulkhead::{Bulkhead, Slot};
use crate::call::{Again, Call, Once};
use crate::config::{LONGEST_ATTEMPT, ResilienceConfig, is_open_time};
use crate::counters::{Counters, Observed};
use crate::error::{BreakerError, Refusal, ResilienceError, TIMED_OUT};
use crate::events::{ResilienceEvent, ResilienceEvents, Subscribers};
use crate::millis::millis;
use crate::rate::{Arrived, Rate, Reservation};
use crate::resilient::{Judge, ResilientAnswer, ResilientMethod, judging_by_default};
use crate::savepoint::{Ended, Savepoint, Settling};
use crate::told::told;

/// The calls to one dependency — a payment provider, a carrier's API, a
/// language model — run under one policy, by every caller in the process.
///
/// Injected as `Arc<ResiliencePolicy<I>>` once a module imports
/// [`ResilienceModule<I>`](crate::ResilienceModule), which builds it from
/// `<PREFIX>_RESILIENCE__<INSTANCE>__*` ([`ResilienceConfig`]); the default
/// instance, `ResiliencePolicy`, reads `<PREFIX>_RESILIENCE__*`. A method runs
/// its body under it through [`#[resilient]`](macro@crate::resilient), naming
/// the field that holds it, and reads its attempt's [`CancellationToken`]
/// under the name its `cancel` key gives; [`run`](Self::run) and
/// [`run_idempotent`](Self::run_idempotent) take a call that is part of a
/// method, as a closure handed that token.
///
/// Each attempt is given up past the policy's timeout or the deadline of the
/// unit it runs in (`nest_rs_core::Deadline`), whichever comes first, and its
/// [`CancellationToken`] cancelled, so work the call handed to another task
/// stops with it; no retry starts whose wait ends past that deadline. A call
/// under one policy made from an attempt of another — a decorated method
/// calling a second dependency's — is given up with that attempt, when that
/// one's bound ends first: its own attempt, in flight there, counts as timed
/// out in its own breaker, so a dependency that never answers opens the
/// breaker naming it however short the timeout its caller runs under, and
/// what its run held unsent — a slot, a token, a probe's turn — is given
/// back, counted for nothing. A call is attempted again only when it is declared
/// idempotent, through [`run_idempotent`](Self::run_idempotent): replaying one
/// that is not would charge a card twice. An attempt's success or failure
/// counts in the circuit breaker, which refuses every call at once while it is
/// open, the bulkhead bounds how many calls run inside the policy together,
/// and the rate limit how many attempts it sends per window, a retry included.
///
/// Inside a unit of work holding a transaction, each attempt of a call
/// declared idempotent, and of a decorated method declaring a `fallback`,
/// sent once or again, runs in a savepoint of it, sent with the attempt's
/// first statement and settled before the next attempt and before the run
/// answers: kept when the run answers the attempt's `Ok`, rolled back when the
/// attempt failed, was given up at its bound or panicked. A failed attempt so
/// leaves no write for the next to repeat, and a run ending on an error leaves
/// its unit what it wrote before the call — whatever retries the policy has,
/// none included: what a call leaves is read off how it is declared, never off
/// a setting of the deployment's. Any other call sent once, and a run on the
/// pool, open none: an `Err` there leaves what its body wrote, as it does with
/// no policy.
///
/// Every `Err` a call run through [`run`](Self::run) or
/// [`run_idempotent`](Self::run_idempotent) answers counts as the dependency
/// failing, and is sent again when the call is idempotent: an answer the
/// dependency gave — a declined card, a parcel not found — belongs in such a
/// call's `Ok`, so it is neither retried nor counted a failure. A decorated
/// method says otherwise of its own answers, key by key — `retry_if`,
/// `retry_ok_if` and `record_if`, an `Err` `record_if` refuses counting in the
/// breaker as neither a failure nor a success — and names the `fallback`
/// answering in place of the error its run ends on. The error is a
/// [`std::error::Error`]: its chain is the `error` of the line a retry files,
/// at `debug`, and of the `warn` the breaker files when that failure opens it,
/// where an operator reads what the dependency answered once every caller is
/// told the breaker is open. A URL in that chain — whatever follows a `://`,
/// up to the next whitespace, control character, double quote or angle
/// bracket — is told by its scheme, its host and a port of digits, and by its
/// scheme alone where an `@` past its authority's end leaves its host
/// impossible to tell from a userinfo: no byte of the userinfo, the path, the
/// query or the fragment a request carried. A status reqwest's own sentence
/// says is told by its class and its code, and the chain no further: the
/// phrase reqwest writes after the code is the dependency's. A value an error
/// quotes otherwise is the error's to keep out, a URL holding one of those
/// four unencoded included, which is read as ending there.
///
/// The breaker's state is read through [`breaker_state`](Self::breaker_state),
/// and switched by hand — an operator taking a dependency out for its
/// maintenance window — through [`open_breaker`](Self::open_breaker) and
/// [`close_breaker`](Self::close_breaker). What the policy holds now is read
/// beside it — [`breaker_stats`](Self::breaker_stats),
/// [`concurrency_running`](Self::concurrency_running),
/// [`concurrency_waiting`](Self::concurrency_waiting),
/// [`rate_available`](Self::rate_available) — and what it does is told as it
/// happens to each subscription [`events`](Self::events) opens.
///
/// ```
/// use std::sync::Arc;
///
/// use nest_rs_core::{App, injectable, instance, module};
/// use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy};
///
/// #[instance]
/// pub struct Upstream;
///
/// #[injectable]
/// pub struct PreviewService {
///     #[inject]
///     upstream: Arc<ResiliencePolicy<Upstream>>,
/// }
///
/// impl PreviewService {
///     pub async fn preview(&self, url: &str) -> Result<String, ResilienceError<std::io::Error>> {
///         self.upstream
///             .run_idempotent(|_cancel| async move { Ok(format!("a preview of {url}")) })
///             .await
///     }
/// }
///
/// #[module(imports = [ResilienceModule<Upstream>], providers = [PreviewService])]
/// pub struct PostsModule;
///
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// let app = App::builder().module::<PostsModule>().build().await?;
/// let service = app.container().get::<PreviewService>().expect("provided");
/// assert_eq!(service.preview("https://example.com").await?, "a preview of https://example.com");
/// # Ok(())
/// # }
/// ```
pub struct ResiliencePolicy<I = Unnamed> {
    timeout: Duration,
    retries: u32,
    backoff: Backoff,
    // Each behind an `Arc`, so the instruments observed off it hold it weakly.
    breaker: Option<Arc<Breaker>>,
    bulkhead: Option<Arc<Bulkhead>>,
    rate: Option<Arc<Rate>>,
    counters: Arc<Counters>,
    subscribers: Arc<Subscribers>,
    observed: Observed,
    /// Sets each run's jitter apart from the others'.
    runs: AtomicU64,
    _instance: PhantomData<fn() -> I>,
}

/// How an attempt's answer counts, on two axes: in the breaker — a success,
/// a failure, or neither — and whether the run sends its call again after it,
/// when it may. An `Ok` is a success, and an `Err` a failure or neither: one
/// variant for each pair that can be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// An `Ok`, the run's answer: a success, not sent again.
    Succeeded,
    /// An `Ok` the run sends its call again after: a success, with no error
    /// to tell and no wait asked.
    Again,
    /// The dependency failed, and the run sends its call again, after the
    /// wait it asked when it said one.
    Failed {
        asked: Option<Duration>,
        /// What it failed on, as the lines telling the attempt say it, said
        /// once where the answer is judged: an error's chain as the policy
        /// tells one ([`told`]).
        error: Cow<'static, str>,
    },
    /// The dependency failed, and the run ends there.
    Final { error: Cow<'static, str> },
    /// An `Err` counting in neither direction, the run's answer: the breaker
    /// holds nothing of it, and the run ends there.
    Ignored,
    /// An `Err` counting in neither direction, which the run sends its call
    /// again after, after the wait it asked when it said one: the breaker
    /// holds nothing of it, and the retry's line tells its `error`.
    IgnoredAgain {
        asked: Option<Duration>,
        error: Cow<'static, str>,
    },
}

impl Verdict {
    /// The attempt `sent` counted in its breaker as the verdict says: a
    /// success, a failure saying what it failed on, or nothing.
    fn settle(&self, sent: Sent<'_>) {
        match self {
            Self::Succeeded | Self::Again => sent.settle(None),
            Self::Failed { error, .. } | Self::Final { error } => sent.settle(Some(error)),
            Self::Ignored | Self::IgnoredAgain { .. } => sent.ignore(),
        }
    }
}

/// What an attempt whose call unwound failed on; its payload is its unit's to file.
pub(crate) const PANICKED: &str = "the call panicked";

// The message of each line a policy files on `TARGET`, its breaker's transition
// apart: what happened, then what the call does, which names the constant.

/// A retry, certain, after an attempt that timed out or answered an `Err`.
pub(crate) const RETRYING_FAILED: &str = "attempt failed; retrying the call";

/// A retry, certain, after an attempt that answered an `Ok` sent again.
pub(crate) const RETRYING_ANSWERED: &str = "attempt answered; retrying the call";

/// A retry given up: the dependency asked a wait past the backoff's ceiling.
pub(crate) const NOT_ATTEMPTED_AGAIN_PAST_CEILING: &str =
    "the dependency asked a wait past the backoff's ceiling; not attempted again";

/// A retry given up: its wait ends past the unit's deadline. Its `error` is
/// what the attempt failed on, an `Err`'s chain or the timeout's sentence, and
/// it has none after an `Ok` that was to be sent again.
pub(crate) const NOT_ATTEMPTED_AGAIN_PAST_DEADLINE: &str =
    "no time left before the unit's deadline; not attempted again";

/// A run ended on its attempt's savepoint, which the database could not settle.
pub(crate) const NOT_ATTEMPTED_AGAIN_UNSETTLED: &str =
    "an attempt's savepoint could not be settled; not attempted again";

/// An attempt waiting for the token of the rate it reserved.
pub(crate) const WAITING_FOR_TOKEN: &str = "rate limit reached; waiting for a token";

/// A call the bulkhead refuses ([`Refusal::Full`]).
pub(crate) const REFUSED_FULL: &str = "bulkhead full; the call is refused";

/// A call the rate refuses ([`Refusal::Limited`]).
pub(crate) const REFUSED_LIMITED: &str = "rate limit reached; the call is refused";

/// A fallback answering in place of the error its run ended on.
pub(crate) const FALLBACK_ANSWERS: &str = "the call ended on an error; its fallback answers";

/// One attempt's answer and how it counts, or the policy's refusal.
pub(crate) type Attempted<T> = Result<(T, Verdict), Refusal>;

/// One run of a call under its policy, holding its bulkhead slot.
pub(crate) struct Run<'p, I> {
    /// What each of its attempts is sent under.
    terms: Terms<'p, I>,
    _slot: Option<Slot<'p>>,
    /// How many times the run may send its call again: the policy's retries,
    /// or the ones its method pinned.
    retries: u32,
    /// Drawn at the first retry, so a call answering at once draws none.
    seed: Option<u64>,
    /// How many times this run attempted its call again so far, a retry the
    /// policy refused unsent included.
    pub(crate) resent: u32,
    /// Whether each attempt runs in a savepoint of its unit's transaction: the
    /// call is declared idempotent, or its method declares a fallback. A
    /// transport's run opens none.
    savepoints: bool,
}

/// What unwinds out of an attempt whose call panicked.
type Unwound = Box<dyn std::any::Any + Send>;

/// How an attempt ended: what it answers, or what its call unwound with.
type Ran<T> = Result<Attempted<T>, Unwound>;

/// What every attempt of one run is sent under, copied into each attempt, so
/// that no attempt's future borrows its run.
pub(crate) struct Terms<'p, I> {
    policy: &'p ResiliencePolicy<I>,
    /// The deadline of the unit the run is in, which no attempt and no wait
    /// outlives.
    deadline: Option<Instant>,
    /// How long each attempt waits: the policy's timeout, or the one its
    /// method pinned.
    timeout: Duration,
    /// The decorated method the run came from, which its lines name.
    method: Option<ResilientMethod>,
}

impl<I> Clone for Terms<'_, I> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<I> Copy for Terms<'_, I> {}

impl<I: Instance> ResiliencePolicy<I> {
    /// The policy `config` describes, refusing a combination of its settings
    /// that cannot work, named as the instance's variables: a first backoff
    /// above the ceiling, and a breaker opening on more failures than its
    /// window holds or, under an age and a rate, than the policy's own
    /// attempts can count within the age of one another — each counted within
    /// the longest attempt of taking its token. That last check holds for what
    /// the policy bounds: the body of a response
    /// [`ResilienceMiddleware`](crate::ResilienceMiddleware) hands its caller
    /// counts when it ends, however long it is held, and is outside it.
    pub(crate) fn new(config: &ResilienceConfig) -> Result<Self, ConfigError> {
        let namespace = instance_namespace::<I>(ResilienceConfig::NAMESPACE);
        if config.backoff_first > config.backoff_ceiling {
            return Err(ConfigError::parse(
                ResilienceConfig::BACKOFF_FIRST.var(&namespace),
                format!(
                    "must be at most {}: a first wait above the longest one is never waited",
                    ResilienceConfig::BACKOFF_CEILING.var_or_field(&namespace),
                ),
            ));
        }
        if let Some(window) = config.breaker_window
            && config.breaker_failures > window
        {
            return Err(ConfigError::parse(
                ResilienceConfig::BREAKER_FAILURES.var(&namespace),
                format!(
                    "must be at most {}: a breaker opening on more failures than its window \
                     holds never opens",
                    ResilienceConfig::BREAKER_WINDOW.var_or_field(&namespace),
                ),
            ));
        }
        let rate = config
            .rate_limit
            .map(|limit| Arc::new(Rate::new(limit, config.rate_window, config.rate_wait)));
        // An attempt's outcome ages from its answer, at most the longest attempt after its
        // token. A body `ResilienceMiddleware` holds counts when it ends, tied to no token.
        if let (Some(_), Some(age), Some(rate)) = (config.breaker_window, config.breaker_age, &rate)
            && config.breaker_failures > rate.most_sent_within(age + LONGEST_ATTEMPT)
        {
            return Err(ConfigError::parse(
                ResilienceConfig::BREAKER_FAILURES.var(&namespace),
                format!(
                    "must be at most the failures the policy's attempts can count within the age \
                     of one another under the rate limit, set by {}; {}; and {}: each attempt \
                     counts within its timeout of taking its token, so a breaker opening on more \
                     is opened by none of them; the body of a response ResilienceMiddleware's \
                     caller holds counts when it ends, outside this bound",
                    ResilienceConfig::BREAKER_AGE.var_or_field(&namespace),
                    ResilienceConfig::RATE_LIMIT.var_or_field(&namespace),
                    ResilienceConfig::RATE_WINDOW.var_or_field(&namespace),
                ),
            ));
        }
        let retries = config
            .retries
            .map_or(0, |retries| u32::try_from(retries).unwrap_or(u32::MAX));
        let counters = Arc::new(Counters::new(I::NAME));
        let subscribers = Arc::<Subscribers>::default();
        let breaker = config.breaker_window.map(|window| {
            Arc::new(Breaker::new(
                I::NAME,
                window,
                config.breaker_failures,
                config.breaker_open,
                config.breaker_age,
                config.breaker_probes,
                Arc::clone(&counters),
                Arc::clone(&subscribers),
            ))
        });
        let bulkhead = config.concurrency.map(|slots| {
            Arc::new(Bulkhead::new(
                slots,
                config.concurrency_wait,
                config.concurrency_waiters,
            ))
        });
        Ok(Self {
            timeout: config.timeout,
            retries,
            backoff: Backoff::new(config.backoff_first, config.backoff_ceiling),
            observed: Observed::new(I::NAME, breaker.as_ref(), bulkhead.as_ref(), rate.as_ref()),
            breaker,
            bulkhead,
            rate,
            counters,
            subscribers,
            runs: AtomicU64::new(0),
            _instance: PhantomData,
        })
    }

    /// Run `call` once, under the policy's timeout, breaker, bulkhead and rate
    /// limit, and never again: a call this runs may change what it calls, so a
    /// retry could do it twice.
    ///
    /// `call` is handed the attempt's [`CancellationToken`], cancelled when the
    /// attempt is given up. Its error is a [`std::error::Error`], whose chain
    /// the breaker's opening tells when that failure opens it.
    pub fn run<T, E, F, Fut>(&self, call: F) -> impl Future<Output = Result<T, ResilienceError<E>>>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: Error + 'static,
    {
        self.run_once::<Result<T, ResilienceError<E>>, _, F, Fut>(None, judging_by_default(), call)
    }

    /// Run the idempotent `call`, attempting it again after a failed or timed
    /// out attempt, up to the policy's retries, each retry waiting the
    /// policy's backoff first — or longer, the `Retry-After` of a
    /// [`Problem`] its error's chain carries. An attempt the circuit breaker
    /// refuses ends the run at once, as does one finding no token of the rate
    /// limit free that cannot wait for the next.
    ///
    /// Idempotent means one call or several leave the dependency as one does:
    /// a read, a `PUT`, a request carrying an idempotency key the dependency
    /// honours. `call` is handed each attempt's [`CancellationToken`],
    /// cancelled when that attempt is given up. Its error is a
    /// [`std::error::Error`], whose chain each retry's line tells, and the
    /// breaker's opening when that failure opens it.
    ///
    /// Inside a unit of work holding a transaction, each attempt runs in a
    /// savepoint of that transaction, whether or not the policy has retries:
    /// what `call` writes through the data layer is rolled back unless the run
    /// answers that attempt's `Ok`. Once an attempt sent a statement, its
    /// savepoint holds the unit until it settles, so another statement of that
    /// unit, issued beside the run, waits for it; an attempt given up while one
    /// of its statements runs is rolled back once that statement ends, which
    /// the next attempt and the run's answer wait for, and one given up before
    /// its savepoint was answered — its unit waiting for a connection, or for
    /// that answer — leaves the unit as it was; and a run dropped mid-attempt,
    /// once its savepoint was answered, leaves what holds that savepoint
    /// unable to commit, as an atomic block dropped does: its unit — or, for a
    /// run nested in another run's attempt or in an atomic block, that
    /// attempt's savepoint or that block alone, which is rolled back while
    /// the unit goes on to commit. A savepoint the database cannot settle
    /// ends the run on [`ResilienceError::Unsettled`].
    ///
    /// An attempt's wait for its unit counts against its bound: of two such
    /// calls joined in one transaction, the second's first statement waits
    /// for the first's whole attempt, and may be given up at its timeout and
    /// counted as its dependency's failure — send them one after the other,
    /// or outside the transaction.
    pub fn run_idempotent<T, E, F, Fut>(
        &self,
        call: F,
    ) -> impl Future<Output = Result<T, ResilienceError<E>>>
    where
        F: FnMut(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: Error + 'static,
    {
        self.run_again::<Result<T, ResilienceError<E>>, _, F, Fut>(None, judging_by_default(), call)
    }

    /// `call` run once, for `method` when a decorated method runs it, its
    /// answer read as `R` reads a body's and answered as `R`, an `Err` a
    /// failure in the breaker when `judging` records it and counted in
    /// neither direction otherwise: what [`run`](Self::run) runs, with no
    /// method and every `Err` a failure.
    pub(crate) fn run_once<R, J, F, Fut>(
        &self,
        method: Option<ResilientMethod>,
        judging: J,
        call: F,
    ) -> impl Future<Output = R>
    where
        R: ResilientAnswer,
        R::Err: Error + 'static,
        J: Judge<R>,
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = R::Attempt>,
    {
        RunOnce {
            stage: RunStage::Opening,
            call: Once::new(call),
            judge: move |answer: &Result<R::Ok, R::Err>| judged_once::<R, J>(&judging, answer),
            attempt: Attempt::new(),
            settling: None,
            wait: Wait::within(),
            run: None,
            policy: self,
            method,
            enter: Self::enter,
            answer: PhantomData,
        }
    }

    /// The idempotent `call` run, for `method` when a decorated method runs
    /// it, each answer read as `R` reads a body's and the last answered as
    /// `R`, each counted as `judging` says — an `Err` or an `Ok` sent again
    /// when it retries it, an `Err` a failure in the breaker when it records
    /// it and counted in neither direction otherwise: what
    /// [`run_idempotent`](Self::run_idempotent) runs, with no method, every
    /// `Err` a failure sent again.
    pub(crate) fn run_again<R, J, F, Fut>(
        &self,
        method: Option<ResilientMethod>,
        judging: J,
        call: F,
    ) -> impl Future<Output = R>
    where
        R: ResilientAnswer,
        R::Err: Error + 'static,
        J: Judge<R>,
        F: FnMut(CancellationToken) -> Fut,
        Fut: Future<Output = R::Attempt>,
    {
        RunAgain {
            stage: RunStage::Opening,
            call: Again::new(call),
            judge: move |answer: &Result<R::Ok, R::Err>| judged_again::<R, J>(&judging, answer),
            attempt: Attempt::new(),
            settling: None,
            wait: Wait::within(),
            run: None,
            policy: self,
            method,
            enter: Self::enter,
            answer: PhantomData,
        }
    }

    /// One run opened as a run's future opens it: [`open`](Self::open), then,
    /// finding no free slot, [`enter`](Self::enter).
    #[cfg(feature = "reqwest")]
    pub(crate) async fn begin(
        &self,
        method: Option<ResilientMethod>,
    ) -> Result<Run<'_, I>, Refusal> {
        match self.open(method) {
            Opened::Run(run) => Ok(run),
            Opened::Refused(refusal) => Err(refusal),
            Opened::Full(bulkhead, deadline) => self.enter(bulkhead, method, deadline).await,
        }
    }

    /// Open one run — of `method`, when a decorated method opens it — under
    /// the unit's deadline, taking a free slot of the bulkhead, refused at
    /// once, in an attempt's order, by a unit out of time and then by an open
    /// breaker; or the bulkhead with no free slot, which the run waits for
    /// through [`enter`](Self::enter).
    fn open(&self, method: Option<ResilientMethod>) -> Opened<'_, I> {
        self.observed.at_run();
        let deadline = nest_rs_core::__private::deadline();
        let slot = match &self.bulkhead {
            Some(bulkhead) => {
                if deadline.is_some_and(|deadline| deadline <= Instant::now()) {
                    return Opened::Refused(self.refused(method.as_ref(), Refusal::OUT_OF_TIME));
                }
                if let Some(retry_after) = self.breaker.as_deref().and_then(Breaker::refusing) {
                    let open = Refusal::Open(retry_after);
                    return Opened::Refused(self.refused(method.as_ref(), open));
                }
                let Some(free) = bulkhead.try_enter() else {
                    return Opened::Full(bulkhead, deadline);
                };
                Some(free)
            }
            None => None,
        };
        Opened::Run(self.opened(method, deadline, slot))
    }

    /// The run of `method` [`open`](Self::open) found no free slot of
    /// `bulkhead` for, opened once one is freed within the unit's `deadline`.
    /// The rate is read first, taking nothing: a call no wait it may make
    /// brings to its token — the one behind the attempts waiting, past the
    /// longest it waits for a slot and the rate's wait from there, or at the
    /// unit's deadline — is refused at once rather than after its wait for a
    /// slot. Any other waits, and meets the rate as it stands once it holds
    /// its slot: refused there by a token still past the rate's wait. It is
    /// refused `Full` once no slot is freed for it in time or as many calls
    /// wait for one as may.
    async fn enter<'p>(
        &'p self,
        bulkhead: &'p Bulkhead,
        method: Option<ResilientMethod>,
        deadline: Option<Instant>,
    ) -> Result<Run<'p, I>, Refusal> {
        if let Some(rate) = &self.rate
            && let Some(wait) = rate.refusing(Instant::now(), bulkhead.longest_wait(), deadline)
        {
            refused_by_the_rate(I::NAME, method.as_ref(), wait);
            return Err(self.refused(method.as_ref(), Refusal::Limited(wait)));
        }
        let slot = bulkhead.enter(deadline).await.map_err(|full| {
            tracing::debug!(
                target: TARGET,
                instance = I::NAME,
                host = method.as_ref().map(ResilientMethod::host).as_deref(),
                method = method.as_ref().map(ResilientMethod::name),
                waiters = full.waiters(),
                "{REFUSED_FULL}",
            );
            self.refused(method.as_ref(), Refusal::Full)
        })?;
        Ok(self.opened(method, deadline, Some(slot)))
    }

    /// The run of `method` opened under the unit's `deadline`, holding `slot`.
    fn opened<'p>(
        &'p self,
        method: Option<ResilientMethod>,
        deadline: Option<Instant>,
        slot: Option<Slot<'p>>,
    ) -> Run<'p, I> {
        Run {
            terms: Terms {
                policy: self,
                deadline,
                timeout: method
                    .and_then(|method| method.timeout())
                    .unwrap_or(self.timeout),
                method,
            },
            _slot: slot,
            retries: method
                .and_then(|method| method.retries())
                .unwrap_or(self.retries),
            seed: None,
            resent: 0,
            savepoints: false,
        }
    }

    /// The circuit breaker's state, as a call arriving now meets it, or `None`
    /// for a policy with no breaker (`BREAKER_WINDOW` set to `0`).
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use nest_rs_resilience::{BreakerState, ResiliencePolicy};
    ///
    /// fn retry_after(policy: &ResiliencePolicy) -> Option<Duration> {
    ///     match policy.breaker_state()? {
    ///         BreakerState::Open { retry_after, .. } => Some(retry_after),
    ///         _ => None,
    ///     }
    /// }
    /// ```
    pub fn breaker_state(&self) -> Option<BreakerState> {
        self.breaker.as_deref().map(Breaker::read)
    }

    /// What the circuit breaker's window counts now — its outcomes, and the
    /// failures among them — as a call arriving now meets it, or `None` for a
    /// policy with no breaker (`BREAKER_WINDOW` set to `0`).
    ///
    /// An outcome past [`breaker_age`](ResilienceConfig::breaker_age) counts
    /// no more, and the window is forgotten as the breaker opens: open or
    /// half-open it reads none, and counts afresh once closed.
    pub fn breaker_stats(&self) -> Option<BreakerStats> {
        self.breaker.as_deref().map(Breaker::stats)
    }

    /// How many calls hold a slot of the bulkhead now — each for its whole
    /// run, its retries and their waits included, and through
    /// [`ResilienceMiddleware`](crate::ResilienceMiddleware) until its
    /// response's body ends — or `None` for a policy with no bulkhead
    /// (`CONCURRENCY` unset). A call is counted from the moment its own
    /// future holds its slot: one a freed slot is handed to is read as
    /// [waiting](Self::concurrency_waiting) until it is polled, so at every
    /// instant a call is in one of the two reads and never in both.
    pub fn concurrency_running(&self) -> Option<usize> {
        self.bulkhead.as_deref().map(Bulkhead::running)
    }

    /// How many calls wait for a slot of the bulkhead now, or `None` for a
    /// policy with no bulkhead (`CONCURRENCY` unset): none ever, where a call
    /// finding no free slot does not wait (`CONCURRENCY_WAIT_MS` unset).
    pub fn concurrency_waiting(&self) -> Option<usize> {
        self.bulkhead.as_deref().map(Bulkhead::waiting)
    }

    /// How many tokens of the rate limit are free now — what as many attempts
    /// arriving together would take, none while an attempt waits for one — or
    /// `None` for a policy with no rate limit (`RATE_LIMIT` unset).
    pub fn rate_available(&self) -> Option<usize> {
        self.rate
            .as_deref()
            .map(|rate| rate.available(Instant::now()))
    }

    /// A subscription to what the policy does from now on: each retry, each
    /// attempt given up at its bound, each call refused unsent, each
    /// transition of its breaker and each fallback answering for a run, as a
    /// [`ResilienceEvent`]. Every subscription is told every event; one opened
    /// later is told nothing that came before it.
    ///
    /// With no subscription open, the policy builds no event. A subscription
    /// is this process's, as the policy's state is, and ends once the policy
    /// is dropped.
    ///
    /// ```
    /// use std::sync::Arc;
    ///
    /// use nest_rs_resilience::{BreakerPhase, ResilienceEvent, ResiliencePolicy};
    ///
    /// fn watch(policy: &Arc<ResiliencePolicy>) {
    ///     // The task holds the subscription and not the policy, so it ends
    ///     // with the app.
    ///     let mut events = policy.events();
    ///     tokio::spawn(async move {
    ///         while let Some(event) = events.next().await {
    ///             if let ResilienceEvent::Transition { to, manual: false, .. } = event
    ///                 && to == BreakerPhase::Open
    ///             {
    ///                 // The dependency is failing: page whoever is on call.
    ///             }
    ///         }
    ///     });
    /// }
    /// ```
    pub fn events(&self) -> ResilienceEvents {
        self.subscribers.subscribe()
    }

    /// Open the circuit breaker by hand for `duration`, an open time
    /// [`breaker_open`](ResilienceConfig::breaker_open) takes in code, above
    /// zero and up to an hour: every call is refused unsent, as by an open
    /// breaker, told what is left of it, then the breaker probes the
    /// dependency as after any opening.
    ///
    /// Whatever the breaker was in, `duration` counts from now and replaces
    /// any open time it had, shorter or longer, and its window is forgotten:
    /// a call let through before it changes nothing once it answers — a probe
    /// answering a success closes nothing — and a probe waiting for its token
    /// of the rate loses its turn. When its wait ends it meets the breaker as
    /// a call
    /// arriving then does: refused while the opening lasts, and taking the
    /// next probe's turn once it has passed. Each call's transition is filed as a
    /// `warn`, marked `manual` and carrying the open time it was given, and
    /// counts on the transitions' counter marked manual, whether or not the
    /// breaker was open already.
    ///
    /// The switch is this process's, as the breaker is: each replica keeps its
    /// own, so a dependency taken out for the whole app is switched on every
    /// replica.
    ///
    /// # Errors
    ///
    /// [`BreakerError::Off`] for a policy with no breaker, and
    /// [`BreakerError::OutOfRange`] for a `duration` `breaker_open` would not
    /// take in code, none or one past an hour; either leaves the breaker as it
    /// was.
    pub fn open_breaker(&self, duration: Duration) -> Result<(), BreakerError> {
        let breaker = self.breaker.as_ref().ok_or(BreakerError::Off)?;
        if !is_open_time(duration) {
            return Err(BreakerError::out_of_range());
        }
        breaker.open_manually(duration);
        Ok(())
    }

    /// Close the circuit breaker by hand, forgetting its window: every call
    /// is sent again, and counted afresh.
    ///
    /// Whatever the breaker was in, a call let through before it changes
    /// nothing once it answers: a probe answering a failure opens nothing,
    /// and no such answer counts in the window that follows. A breaker its
    /// probes closed counts the same way: the answer of a call sent before
    /// it opened counts nowhere. Each call's transition is filed as a `warn`,
    /// marked `manual`, and counts on the transitions' counter marked
    /// manual, whether or not the breaker was closed already.
    /// Like [`open_breaker`](Self::open_breaker), it switches this replica's
    /// breaker alone.
    ///
    /// # Errors
    ///
    /// [`BreakerError::Off`] for a policy with no breaker.
    pub fn close_breaker(&self) -> Result<(), BreakerError> {
        self.breaker
            .as_ref()
            .ok_or(BreakerError::Off)?
            .close_manually();
        Ok(())
    }

    /// `refusal`, which a run of `method` ends on, counted and told. It files
    /// no line: the bulkhead's and the rate's file theirs where they refuse.
    fn refused(&self, method: Option<&ResilientMethod>, refusal: Refusal) -> Refusal {
        self.counters.refusal(refusal);
        self.subscribers
            .tell(|| ResilienceEvent::refused(method, refusal));
        refusal
    }

    /// A run of `method` ended on `error`, which its fallback answers in
    /// place of: the line saying so, with the error's chain, counted and
    /// told. The chain is said once, and only for a line filed or a
    /// subscription open.
    pub(crate) fn falling_back(&self, method: &ResilientMethod, error: &(dyn Error + 'static)) {
        let mut said = None;
        tracing::debug!(
            target: TARGET,
            instance = I::NAME,
            host = method.host(),
            method = method.name(),
            error = said.insert(told(error)).as_str(),
            "{FALLBACK_ANSWERS}",
        );
        self.counters.fallback();
        self.subscribers
            .tell(|| ResilienceEvent::fallback(method, said.unwrap_or_else(|| told(error))));
    }
}

impl<'p, I: Instance> Run<'p, I> {
    /// Whether the run may still send its call again.
    #[cfg(feature = "reqwest")]
    pub(crate) const fn may_resend(&self) -> bool {
        self.resent < self.retries
    }

    /// One attempt of `call`, holding the call itself, its answer counted as
    /// `judge` reads it: in the breaker at once when it failed, and by what
    /// the attempt leaves [`Outstanding`] when `judge` read a success, which
    /// what the call answered may still undo — a response's body.
    #[cfg(feature = "reqwest")]
    pub(crate) fn attempt_once<T, E, F, Fut, J>(
        &self,
        call: F,
        judge: J,
    ) -> impl Future<Output = (Attempted<Result<T, E>>, Outstanding<'p>)> + use<'p, '_, I, T, E, F, Fut, J>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
        J: Fn(&Result<T, E>) -> Verdict,
    {
        AttemptOnce::<'p, '_, I, Result<T, ResilienceError<E>>, F, Fut, J> {
            call: Once::new(call),
            judge,
            attempt: Attempt::new(),
            sent_at: None,
            wait: Wait::within(),
            run: self,
            answer: PhantomData,
        }
    }

    /// The run over at an answer a part of which is still to come, what it
    /// holds of `policy`, the one it runs under, handed to that part: its slot
    /// of the bulkhead, and what its last attempt left `outstanding`.
    #[cfg(feature = "reqwest")]
    pub(crate) fn hold(
        self,
        mut outstanding: Outstanding<'p>,
        policy: Arc<ResiliencePolicy<I>>,
    ) -> Held<I> {
        let Self {
            terms, _slot: slot, ..
        } = self;
        debug_assert!(
            std::ptr::eq(terms.policy, Arc::as_ptr(&policy)),
            "a run's slot and its attempt are held for the policy they were taken from"
        );
        Held {
            attempt: outstanding
                .sent
                .take()
                .map(|sent| sent.hold(outstanding.bound)),
            slot: slot
                .zip(policy.bulkhead.as_deref())
                .map(|(slot, bulkhead)| bulkhead.hold(slot)),
            left: outstanding.ends.saturating_duration_since(Instant::now()),
            bound: outstanding.bound,
            policy,
        }
    }

    /// The run's last attempt, its refusal counted and told when it ended on
    /// one — as the attempt ends, before its savepoint settles: an edge
    /// cutting the unit at the deadline the attempt was given up at drops the
    /// run while it rolls back, and the breaker has counted that timeout.
    pub(crate) fn finish<T>(&self, attempted: Attempted<T>) -> Attempted<T> {
        let terms = &self.terms;
        attempted.map_err(|refusal| terms.policy.refused(terms.method.as_ref(), refusal))
    }

    /// The wait before the call is attempted again after `attempted`, as
    /// [`next_wait`](Self::next_wait) decides it, or `attempted` handed back
    /// when the run ends there. An answer sent again is let go here, as its
    /// retry is decided: the wait is handed nothing of it, so none holds what
    /// the answer does — a response's connection, or its stream.
    #[cfg(any(feature = "reqwest", test))]
    pub(crate) fn again<'r, T>(
        &'r mut self,
        attempted: Attempted<T>,
    ) -> Result<impl Future<Output = ()> + use<'r, 'p, I, T>, Attempted<T>> {
        let Some(wait) = self.next_wait(&attempted) else {
            return Err(attempted);
        };
        drop(attempted);
        Ok(async move {
            tokio::time::sleep(wait).await;
            self.resent = self.resent.saturating_add(1);
        })
    }

    /// How long to wait before attempting the call again after `attempted`, the
    /// retry's line filed, or `None` when the run ends there: [`retry`]'s
    /// decision for a run opening no savepoint, whose retry is certain as it
    /// is decided.
    ///
    /// It keeps nothing of the answer: [`again`](Self::again) lets it go
    /// before the wait, and a run stays `Send` for an answer that is `Send`
    /// and not `Sync`, a stream's.
    ///
    /// [`retry`]: Self::retry
    #[cfg(any(feature = "reqwest", test))]
    fn next_wait<T>(&mut self, attempted: &Attempted<T>) -> Option<Duration> {
        let (asked, error) = sent_again(attempted)?;
        let wait = self.retry_wait(asked, error)?;
        self.given_up(attempted);
        self.retrying(wait, error);
        Some(wait)
    }

    /// The retry the run makes after `attempted`, or `None` when it ends
    /// there: after a timeout or an answer judged one to send again, as
    /// [`retry_wait`] decides. Its line is left to file once the retry is
    /// certain ([`retrying`]): at once for an attempt that opened no
    /// savepoint, else once that savepoint was rolled back. What the attempt
    /// answered in error is taken out of `attempted`, which the run lets go
    /// before it waits.
    ///
    /// [`retry_wait`]: Self::retry_wait
    /// [`retrying`]: Self::retrying
    fn retry<T>(&mut self, attempted: &mut Attempted<T>) -> Option<Retry> {
        let (asked, error) = sent_again(attempted)?;
        let wait = self.retry_wait(asked, error)?;
        self.given_up(attempted);
        let error = match attempted {
            Ok((_, Verdict::Failed { error, .. } | Verdict::IgnoredAgain { error, .. })) => {
                Some(std::mem::take(error))
            }
            Ok((_, Verdict::Again)) => None,
            _ => Some(Cow::Borrowed(TIMED_OUT)),
        };
        Some(Retry { wait, error })
    }

    /// An attempt given up at its bound, which the run sends its call again
    /// after, told: its run ends on no refusal, where one is told otherwise
    /// ([`ResiliencePolicy::refused`]).
    fn given_up<T>(&self, attempted: &Attempted<T>) {
        if let Err(timed_out @ Refusal::TimedOut(_)) = attempted {
            let method = self.terms.method.as_ref();
            self.terms
                .policy
                .subscribers
                .tell(|| ResilienceEvent::refused(method, *timed_out));
        }
    }

    /// The line of a retry that is certain, saying what the attempt before it
    /// answered: `attempt failed`, with its error's chain or the timeout's
    /// sentence, for an `Err` or a timeout, whichever way the `Err` counts in
    /// the breaker, and `attempt answered`, with no error, for an `Ok` sent
    /// again. The retry is told as it is filed.
    fn retrying(&self, wait: Duration, error: Option<&str>) {
        let attempt = self.resent.saturating_add(1);
        match error {
            Some(error) => tracing::debug!(
                target: TARGET,
                instance = I::NAME,
                host = self.terms.host().as_deref(),
                method = self.terms.method_name(),
                attempt,
                wait_ms = millis(wait),
                error,
                "{RETRYING_FAILED}",
            ),
            None => tracing::debug!(
                target: TARGET,
                instance = I::NAME,
                host = self.terms.host().as_deref(),
                method = self.terms.method_name(),
                attempt,
                wait_ms = millis(wait),
                "{RETRYING_ANSWERED}",
            ),
        }
        self.terms
            .policy
            .subscribers
            .tell(|| ResilienceEvent::retry(self.terms.method.as_ref(), attempt, wait, error));
    }

    /// How long to wait before the next attempt, after one that answered
    /// `error` asking `asked`, or `None` when the run ends there: its retries
    /// spent, or a retry left that is given up, whose line says why — the
    /// dependency asked a wait past the backoff's ceiling, or the wait ends
    /// past the unit's deadline — with `error`, which an `Ok` sent again has
    /// none of.
    fn retry_wait(&mut self, asked: Option<Duration>, error: Option<&str>) -> Option<Duration> {
        let terms = self.terms;
        let policy = terms.policy;
        if self.resent >= self.retries {
            return None;
        }
        let ceiling = policy.backoff.ceiling();
        if let Some(asked) = asked.filter(|asked| *asked > ceiling) {
            tracing::debug!(
                target: TARGET,
                instance = I::NAME,
                host = terms.host().as_deref(),
                method = terms.method_name(),
                asked_ms = millis(asked),
                ceiling_ms = millis(ceiling),
                error,
                "{NOT_ATTEMPTED_AGAIN_PAST_CEILING}",
            );
            return None;
        }
        let seed = *self.seed.get_or_insert_with(|| {
            let run = policy.runs.fetch_add(1, Ordering::Relaxed);
            process_seed() ^ Backoff::seed(&run.to_le_bytes())
        });
        let failed = NonZeroU32::MIN.saturating_add(self.resent);
        let backoff = policy.backoff.wait(failed, seed);
        let wait = asked.map_or(backoff, |asked| {
            backoff.max(Backoff::honouring(asked, failed, seed))
        });
        if let Some(deadline) = terms.deadline
            && Instant::now() + wait >= deadline
        {
            tracing::debug!(
                target: TARGET,
                instance = I::NAME,
                host = terms.host().as_deref(),
                method = terms.method_name(),
                wait_ms = millis(wait),
                error,
                "{NOT_ATTEMPTED_AGAIN_PAST_DEADLINE}",
            );
            return None;
        }
        Some(wait)
    }
}

/// What an attempt whose call answered leaves outstanding with its policy:
/// the attempt in the breaker, still to settle when its answer was judged a
/// success, and its bound, which runs from its send. Dropped, the answer stands
/// as it was judged; [`Run::hold`] hands it to what the answer still owes.
#[cfg(feature = "reqwest")]
pub(crate) struct Outstanding<'p> {
    sent: Option<Sent<'p>>,
    /// The instant the attempt's bound ends at.
    ends: Instant,
    bound: Duration,
}

#[cfg(feature = "reqwest")]
impl Drop for Outstanding<'_> {
    fn drop(&mut self) {
        if let Some(sent) = self.sent.take() {
            sent.settle(None);
        }
    }
}

/// What a run's answer holds of its policy once the run is over, while a part
/// of it is still to come — a response's body: the run's slot of the bulkhead,
/// its last attempt in the breaker when its answer was judged a success, and
/// what that attempt's bound had left as the run ended. It borrows nothing, so
/// it lives as long as what it covers; dropped before it is settled, the
/// answer stands as it was judged.
#[cfg(feature = "reqwest")]
pub(crate) struct Held<I> {
    policy: Arc<ResiliencePolicy<I>>,
    attempt: Option<HeldAttempt>,
    slot: Option<HeldSlot>,
    left: Duration,
    bound: Duration,
}

#[cfg(feature = "reqwest")]
impl<I> Held<I> {
    /// What the attempt's bound, counted from its send, had left as the run
    /// ended: how long the rest of the answer may still be waited for.
    pub(crate) const fn left(&self) -> Duration {
        self.left
    }

    /// The answer's outcome now that it is whole, or will never be: `failure`
    /// saying what it failed on, or `None` for the success it was judged. The
    /// breaker counts it and the slot is freed, once: settled again, it
    /// changes nothing.
    pub(crate) fn settle(&mut self, failure: Option<&str>) {
        let policy = &*self.policy;
        // Freed after the breaker, as a run's drop hands a probe's turn on before
        // its slot — and as the breaker's telling unwinds, should a line panic.
        let _slot = self
            .slot
            .take()
            .zip(policy.bulkhead.as_deref())
            .map(|(slot, bulkhead)| bulkhead.resume(slot));
        if let Some((attempt, breaker)) = self.attempt.take().zip(policy.breaker.as_deref()) {
            breaker.resume(attempt).settle(failure);
        }
    }
}

#[cfg(feature = "reqwest")]
impl<I: Instance> Held<I> {
    /// The answer given up, its reader having waited for the rest of it all
    /// its attempt's bound had left: a failure, timed out, and the refusal its
    /// reader is told, counted.
    pub(crate) fn timed_out(&mut self) -> Refusal {
        self.settle(Some(TIMED_OUT));
        self.policy.refused(None, Refusal::TimedOut(self.bound))
    }
}

#[cfg(feature = "reqwest")]
impl<I> Held<I> {
    /// Reading the rest of the answer panicked: a failure, as a panic in an
    /// attempt's call is.
    pub(crate) fn panicked(&mut self) {
        self.settle(Some(PANICKED));
    }
}

#[cfg(feature = "reqwest")]
impl<I> Drop for Held<I> {
    fn drop(&mut self) {
        self.settle(None);
    }
}

impl<I> Run<'_, I> {
    /// The run, each of its attempts in a savepoint of its unit's transaction
    /// when its call is declared `idempotent` or its method declares a
    /// fallback: read off the declaration alone, so the retries a deployment
    /// sets never change what a failed call leaves in its unit.
    fn in_savepoints(mut self, idempotent: bool) -> Self {
        let falls_back = self.terms.method.is_some_and(|method| method.fallback());
        self.savepoints = idempotent || falls_back;
        self
    }
}

impl<I: Instance> Terms<'_, I> {
    /// What the run does once an attempt's savepoint settled, `ended` being
    /// what the attempt ended on: its answer to answer, its call to send
    /// again, its panic unwinding on. A savepoint the database could not
    /// settle ends the run there, whatever the attempt answered — a timeout
    /// it was given up at counted and told already, as the attempt's:
    /// nothing that attempt wrote stands. Its line tells why, the cause's
    /// chain as its `error`: a driver owes no line of its own for it, and the
    /// [`Unsettled`](ResilienceError::Unsettled) holding it reaches the run's
    /// caller alone, never past a panic unwinding on.
    fn after_savepoint<T, E>(
        &self,
        ended: Ended<Attempted<Result<T, E>>>,
        settled: Result<(), AtomicError>,
    ) -> Result<Next<Attempted<Result<T, E>>>, ResilienceError<E>> {
        if let Err(cause) = &settled {
            tracing::debug!(
                target: TARGET,
                instance = I::NAME,
                host = self.host().as_deref(),
                method = self.method_name(),
                error = told(cause).as_str(),
                "{NOT_ATTEMPTED_AGAIN_UNSETTLED}",
            );
        }
        match (ended, settled) {
            (Ended::Unwound(unwound), _) => std::panic::resume_unwind(unwound),
            (_, Err(cause)) => Err(ResilienceError::unsettled(cause)),
            (Ended::Answer(attempted), Ok(())) => Ok(Next::Answer(attempted)),
            (Ended::Again(retry), Ok(())) => Ok(Next::Again(retry)),
        }
    }
}

/// What a run goes on with once its attempt's savepoint settled.
enum Next<T> {
    /// The attempt's answer, which the run answers.
    Answer(T),
    /// The retry decided as the attempt failed, certain now.
    Again(Retry),
}

/// A retry decided after an attempt: how long the run waits before it,
/// counted from that attempt's answer, and what the attempt answered in
/// error, which the retry's line says — nothing, for an `Ok` sent again.
pub(crate) struct Retry {
    wait: Duration,
    error: Option<Cow<'static, str>>,
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
pub(crate) struct Bound {
    wait: Duration,
    until: Option<Instant>,
}

impl<I> Terms<'_, I> {
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

    /// The decorated method's host, named as its `mounted` line would name it,
    /// read only when a line is filed.
    fn host(&self) -> Option<String> {
        self.method.as_ref().map(ResilientMethod::host)
    }

    /// The decorated method's name.
    fn method_name(&self) -> Option<&'static str> {
        self.method.as_ref().map(ResilientMethod::name)
    }
}

/// What a run meets as it opens.
enum Opened<'p, I> {
    Run(Run<'p, I>),
    Refused(Refusal),
    /// No free slot of the bulkhead, waited for under the unit's deadline.
    Full(&'p Bulkhead, Option<Instant>),
}

/// Where a run is.
#[derive(Clone, Copy)]
enum RunStage {
    Opening,
    Attempting,
    Answered,
}

/// What an attempt meets as it is let through.
enum Admission<'p> {
    /// Let through, its call sent under the bound, holding the breaker's pass.
    Through(Option<Pass<'p>>, Bound),
    Refused(Refusal),
    /// No token of the rate free, postponed to the instant its reserved one is.
    Postponed(Instant),
}

impl<'p, I: Instance> Terms<'p, I> {
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
    /// timer.
    // Inlined, the rate's step alone left out of line: a run with no rate pays no
    // call to be let through.
    #[inline(always)]
    fn admit(
        &self,
        pass: &mut Option<Pass<'p>>,
        reserved: &mut Option<Reservation<'p>>,
    ) -> Admission<'p> {
        let bound = self.bound();
        if bound.wait.is_zero() {
            return Admission::Refused(Refusal::OUT_OF_TIME);
        }
        // While this one waited for its token, another call may have opened the
        // breaker or taken the probe's turn, and a manual switch voids a turn held.
        if !pass.as_ref().is_some_and(Pass::probes) {
            match self
                .policy
                .breaker
                .as_deref()
                .map(|breaker| breaker.admit(bound.wait))
                .transpose()
            {
                Ok(admitted) => *pass = admitted,
                Err(retry_after) => return Admission::Refused(Refusal::Open(retry_after)),
            }
        }
        match &self.policy.rate {
            Some(rate) => self.token(rate, pass, reserved, bound),
            None => Admission::Through(pass.take(), bound),
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
                refused_by_the_rate(I::NAME, self.method.as_ref(), wait);
                Admission::Refused(Refusal::Limited(wait))
            }
            Arrived::Reserved(reservation, at) => {
                *reserved = Some(reservation);
                if let Some(pass) = pass.as_ref() {
                    pass.postpone(at, self.bound_at(at));
                }
                tracing::debug!(
                    target: TARGET,
                    instance = I::NAME,
                    host = self.host().as_deref(),
                    method = self.method_name(),
                    wait_ms = millis(at.saturating_duration_since(now)),
                    "{WAITING_FOR_TOKEN}",
                );
                Admission::Postponed(at)
            }
        }
    }
}

// The run's futures are polled in place, each holding the call once, where an `async` fn
// or block awaiting a future it was handed keeps it twice (rust-lang/rust#62958).
pin_project_lite::pin_project! {
    /// An attempt that found no token of the rate free, waiting for the one
    /// it reserved, then let through again as [`Terms::admit`] says. Dropped
    /// before then — its run dropped, its breaker refusing it or its unit out
    /// of time once the wait ends — it gives the token to the attempt behind.
    struct Admitted<'p, I> {
        terms: Terms<'p, I>,
        // `reserved` before `pass`: given back first, so the call a probe's turn
        // is handed to finds the token this one left.
        reserved: Option<Reservation<'p>>,
        pass: Option<Pass<'p>>,
        #[pin]
        due: Sleep,
    }
}

impl<'p, I: Instance> Future for Admitted<'p, I> {
    type Output = Result<(Option<Pass<'p>>, Bound), Refusal>;

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
            match this.terms.admit(this.pass, this.reserved) {
                Admission::Through(pass, bound) => return Poll::Ready(Ok((pass, bound))),
                Admission::Refused(refusal) => return Poll::Ready(Err(refusal)),
                Admission::Postponed(at) => this.due.as_mut().reset(at),
            }
        }
    }
}

pin_project_lite::pin_project! {
    /// What a run waits on besides its call, one at a time: the bound of its
    /// attempt, whose timer is armed once the call waits, and only when they
    /// come, the slot of the bulkhead it found none free of, the rate's token
    /// and the backoff before a retry.
    #[project = WaitProj]
    enum Wait<'p, I, B> {
        Within {
            #[pin]
            within: WithinHeld,
        },
        Entering {
            #[pin]
            entering: B,
        },
        Admitting {
            #[pin]
            admitted: Admitted<'p, I>,
        },
        BackingOff {
            #[pin]
            backoff: Sleep,
        },
    }
}

impl<I, B> Wait<'_, I, B> {
    const fn within() -> Self {
        Self::Within {
            within: WithinHeld::new(Duration::ZERO, None),
        }
    }

    /// The attempt's bound, which the run holds whenever it waits on nothing else.
    fn bound(self: Pin<&mut Self>) -> Pin<&mut WithinHeld> {
        let WaitProj::Within { within } = self.project() else {
            unreachable!("an attempt is sent and answers while its run waits on nothing else");
        };
        within
    }
}

/// One attempt of a run's call, polled with its run, what the run waits on
/// and the call it holds: let through as [`Terms::admit`] says, waiting for the
/// rate's token only when it must, then its call made and sent under its
/// bound, counted in the breaker once it answers and its token cancelled when
/// it is given up. Each step writes its fields where they are.
///
/// A `HOLDS` attempt leaves an answer judged a success still to settle in the
/// breaker, its `sent` for what holds the attempt to take: what the call
/// answered may still undo it.
pub(crate) struct Attempt<'p, const HOLDS: bool = false> {
    stage: AttemptStage,
    // `sent` before `given_up`: an attempt dropped mid-call settles the
    // breaker, then cancels its token.
    sent: Option<Sent<'p>>,
    given_up: Option<DropGuard>,
    /// The token the call is handed when it is made, at its first poll.
    cancel: Option<CancellationToken>,
    bound: Duration,
    /// The savepoint the call runs in, which the run settles once the attempt
    /// answered; dropped unsettled with an attempt given up mid-call, it is
    /// rolled back as the port says of a block dropped.
    savepoint: Option<Savepoint>,
}

/// Where an attempt is.
#[derive(Clone, Copy)]
enum AttemptStage {
    /// Not let through yet.
    Arriving,
    /// Waiting for the token of the rate it reserved.
    Admitting,
    /// Its call sent.
    Sending,
    Answered,
}

impl<'p, const HOLDS: bool> Attempt<'p, HOLDS> {
    const fn new() -> Self {
        Self {
            stage: AttemptStage::Arriving,
            sent: None,
            given_up: None,
            cancel: None,
            bound: Duration::ZERO,
            savepoint: None,
        }
    }

    /// The attempt of `run` polled with `call`, its answer read as `R` reads a
    /// body's and counted as `judge` reads it. A panic in the call counts as a
    /// failure and unwinds on, for the unit to contain as it would without the
    /// policy.
    #[cfg(feature = "reqwest")]
    fn poll_call<I, B, C, R, J>(
        &mut self,
        run: &Run<'p, I>,
        wait: Pin<&mut Wait<'p, I, B>>,
        call: Pin<&mut C>,
        judge: &J,
        cx: &mut Context<'_>,
    ) -> Poll<Attempted<Result<R::Ok, R::Err>>>
    where
        I: Instance,
        C: Call,
        C::Made: Future<Output = R::Attempt>,
        R: ResilientAnswer,
        J: Fn(&Result<R::Ok, R::Err>) -> Verdict,
    {
        match ready!(self.poll_ended::<I, B, C, R, J>(run, wait, call, judge, cx)) {
            Ok(attempted) => Poll::Ready(attempted),
            Err(unwound) => std::panic::resume_unwind(unwound),
        }
    }

    /// [`poll_call`](Self::poll_call), a panic in the call handed back once it
    /// counted as a failure, for a run to roll the attempt's savepoint back
    /// before it unwinds on.
    fn poll_ended<I, B, C, R, J>(
        &mut self,
        run: &Run<'p, I>,
        mut wait: Pin<&mut Wait<'p, I, B>>,
        mut call: Pin<&mut C>,
        judge: &J,
        cx: &mut Context<'_>,
    ) -> Poll<Ran<Result<R::Ok, R::Err>>>
    where
        I: Instance,
        C: Call,
        C::Made: Future<Output = R::Attempt>,
        R: ResilientAnswer,
        J: Fn(&Result<R::Ok, R::Err>) -> Verdict,
    {
        loop {
            let admitted = match self.stage {
                AttemptStage::Arriving => {
                    let (mut pass, mut reserved) = (None, None);
                    match run.terms.admit(&mut pass, &mut reserved) {
                        Admission::Through(pass, bound) => Ok((pass, bound)),
                        Admission::Refused(refusal) => Err(refusal),
                        Admission::Postponed(at) => {
                            wait.set(Wait::Admitting {
                                admitted: Admitted {
                                    terms: run.terms,
                                    reserved,
                                    pass,
                                    due: tokio::time::sleep_until(at),
                                },
                            });
                            self.stage = AttemptStage::Admitting;
                            continue;
                        }
                    }
                }
                AttemptStage::Admitting => {
                    let WaitProj::Admitting { admitted } = wait.as_mut().project() else {
                        unreachable!("an attempt waits for the token it reserved");
                    };
                    let admitted = ready!(admitted.poll(cx));
                    wait.set(Wait::within());
                    admitted
                }
                AttemptStage::Sending => {
                    let making = pin!(Making {
                        call: call.as_mut(),
                        cancel: &mut self.cancel,
                    });
                    let bound = wait.as_mut().bound();
                    let answered = ready!(match &self.savepoint {
                        Some(savepoint) => bound.poll(pin!(savepoint.around(making)), cx),
                        None => bound.poll(making, cx),
                    });
                    return Poll::Ready(self.settle::<C, R, J>(call, answered, judge));
                }
                AttemptStage::Answered => unreachable!("an attempt is polled after it answered"),
            };
            match admitted {
                Ok((pass, bound)) => self.send(run, wait.as_mut(), pass, bound),
                Err(refusal) => {
                    self.stage = AttemptStage::Answered;
                    return Poll::Ready(Ok(Err(refusal)));
                }
            }
        }
    }

    /// The savepoint the attempt ran in, for its run to settle.
    fn take_savepoint(&mut self) -> Option<Savepoint> {
        self.savepoint.take()
    }

    /// The attempt let through with `pass`, sent under `bound`: its call is
    /// made at its first poll, inside the bound, so one panicking before it
    /// hands back its future fails the attempt as a panic while it runs does.
    /// A retry counts on the retries' counter.
    fn send<I: Instance, B>(
        &mut self,
        run: &Run<'p, I>,
        wait: Pin<&mut Wait<'p, I, B>>,
        pass: Option<Pass<'p>>,
        bound: Bound,
    ) {
        if run.resent > 0 {
            run.terms.policy.counters.retry();
        }
        self.sent = pass.map(|pass| pass.send(bound.wait, bound.until));
        let cancel = CancellationToken::new();
        self.given_up = Some(cancel.clone().drop_guard());
        self.cancel = Some(cancel);
        self.bound = bound.wait;
        wait.bound().reset(bound.wait, bound.until);
        if run.savepoints {
            self.savepoint = Savepoint::open();
        }
        self.stage = AttemptStage::Sending;
    }

    /// What the attempt answers, `answered` read as `R` reads a body's, judged
    /// and counted in the breaker, the future the call made dropped first, so
    /// before its savepoint settles. `judge` says what a failed answer failed
    /// on before the breaker is asked, so no `Display` of the call's error runs
    /// under its lock; a `judge` that panics — a predicate of the call's —
    /// fails the attempt as a panic in its call does.
    fn settle<C, R, J>(
        &mut self,
        call: Pin<&mut C>,
        answered: Result<R::Attempt, Unanswered>,
        judge: &J,
    ) -> Ran<Result<R::Ok, R::Err>>
    where
        C: Call,
        R: ResilientAnswer,
        J: Fn(&Result<R::Ok, R::Err>) -> Verdict,
    {
        self.stage = AttemptStage::Answered;
        self.cancel = None;
        match &answered {
            Ok(_) | Err(Unanswered::Panicked(_)) => call.spend(),
            // An attempt of another policy in flight in the call is dropped by
            // this timeout, never by its caller.
            Err(_elapsed) => Cut::dropping(|| call.spend()),
        }
        let answered = answered.and_then(|attempt| {
            let answer = R::split(attempt);
            match catch_unwind(AssertUnwindSafe(|| judge(&answer))) {
                Ok(verdict) => Ok((answer, verdict)),
                Err(unwound) => Err(Unanswered::Panicked(unwound)),
            }
        });
        let held = HOLDS && matches!(answered, Ok((_, Verdict::Succeeded)));
        if !held && let Some(sent) = self.sent.take() {
            match &answered {
                Ok((_, verdict)) => verdict.settle(sent),
                Err(Unanswered::Panicked(_)) => sent.settle(Some(PANICKED)),
                Err(_elapsed) => sent.settle(Some(TIMED_OUT)),
            }
        }
        let given_up = self.given_up.take();
        match answered {
            Ok(judged) => {
                let _token = given_up.map(DropGuard::disarm);
                Ok(Ok(judged))
            }
            Err(Unanswered::Panicked(payload)) => Err(payload),
            Err(_elapsed) => Ok(Err(Refusal::TimedOut(self.bound))),
        }
    }
}

/// The future of an attempt's call, made at its first poll with the token
/// the attempt holds until then.
struct Making<'a, C> {
    call: Pin<&'a mut C>,
    cancel: &'a mut Option<CancellationToken>,
}

impl<C: Call> Future for Making<'_, C> {
    type Output = <C::Made as Future>::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        if let Some(cancel) = this.cancel.take() {
            this.call.as_mut().make(cancel);
        }
        this.call.as_mut().made().poll(cx)
    }
}

#[cfg(feature = "reqwest")]
pin_project_lite::pin_project! {
    /// One attempt of `run` holding its call — what [`Run::attempt_once`]
    /// answers.
    struct AttemptOnce<'p, 'r, I, R, F, Fut, J> {
        // In `RunOnce`'s order.
        #[pin]
        call: Once<F, Fut>,
        judge: J,
        attempt: Attempt<'p, true>,
        // Read at each poll until the call is sent: the last is its send's.
        sent_at: Option<Instant>,
        #[pin]
        wait: Wait<'p, I, std::future::Pending<()>>,
        run: &'r Run<'p, I>,
        answer: PhantomData<fn() -> R>,
    }
}

#[cfg(feature = "reqwest")]
impl<'p, I, R, F, Fut, J> Future for AttemptOnce<'p, '_, I, R, F, Fut, J>
where
    I: Instance,
    R: ResilientAnswer,
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future<Output = R::Attempt>,
    J: Fn(&Result<R::Ok, R::Err>) -> Verdict,
{
    type Output = (Attempted<Result<R::Ok, R::Err>>, Outstanding<'p>);

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        let sent_at = match (this.attempt.stage, *this.sent_at) {
            (AttemptStage::Sending, Some(sent_at)) => sent_at,
            _ => *this.sent_at.insert(Instant::now()),
        };
        let attempted = ready!(this.attempt.poll_call::<_, _, _, R, J>(
            this.run,
            this.wait.as_mut(),
            this.call.as_mut(),
            this.judge,
            cx,
        ));
        released(this.call, this.wait);
        let bound = this.attempt.bound;
        let outstanding = Outstanding {
            sent: this.attempt.sent.take(),
            ends: sent_at + bound,
            bound,
        };
        Poll::Ready((attempted, outstanding))
    }
}

pin_project_lite::pin_project! {
    /// What [`ResiliencePolicy::run_once`] answers: a slot of the bulkhead
    /// taken, then the run's one attempt.
    struct RunOnce<'p, I, E, B, R, F, Fut, J>
    where
        R: ResilientAnswer,
    {
        stage: RunStage,
        // Dropped in order: the call's future, then the attempt and the wait, each
        // handing on the turn of a probe it holds, sent or waiting for the rate's
        // token, then the run, freeing its slot.
        #[pin]
        call: Once<F, Fut>,
        judge: J,
        attempt: Attempt<'p>,
        settling: Option<Box<Settling<Attempted<Result<R::Ok, R::Err>>>>>,
        #[pin]
        wait: Wait<'p, I, B>,
        run: Option<Run<'p, I>>,
        policy: &'p ResiliencePolicy<I>,
        method: Option<ResilientMethod>,
        // `ResiliencePolicy::enter`, whose future has no name.
        enter: E,
        answer: PhantomData<fn() -> R>,
    }
}

impl<'p, I, E, B, R, F, Fut, J> Future for RunOnce<'p, I, E, B, R, F, Fut, J>
where
    I: Instance,
    E: Fn(&'p ResiliencePolicy<I>, &'p Bulkhead, Option<ResilientMethod>, Option<Instant>) -> B,
    B: Future<Output = Result<Run<'p, I>, Refusal>>,
    R: ResilientAnswer,
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future<Output = R::Attempt>,
    J: Fn(&Result<R::Ok, R::Err>) -> Verdict,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<R> {
        let mut this = self.project();
        if let RunStage::Opening = *this.stage {
            let opened = ready!(poll_open(
                this.policy,
                *this.method,
                this.enter,
                this.wait.as_mut(),
                cx
            ));
            match opened {
                Ok(run) => {
                    *this.run = Some(run.in_savepoints(false));
                    *this.stage = RunStage::Attempting;
                }
                Err(refusal) => {
                    *this.stage = RunStage::Answered;
                    released(this.call, this.wait);
                    return Poll::Ready(R::answered(Err(refusal.into())));
                }
            }
        }
        let (RunStage::Attempting, Some(run)) = (*this.stage, this.run.as_ref()) else {
            unreachable!("a run is polled after it answered");
        };
        let ran = loop {
            if let Some(settling) = this.settling.as_mut() {
                let (ended, settled_as) = ready!(settling.poll(cx));
                *this.settling = None;
                break match run.terms.after_savepoint(ended, settled_as) {
                    Ok(Next::Answer(attempted)) => settled(attempted),
                    Ok(Next::Again(_)) => unreachable!("a call sent once is not sent again"),
                    Err(unsettled) => Err(unsettled),
                };
            }
            let ended = ready!(this.attempt.poll_ended::<_, _, _, R, J>(
                run,
                this.wait.as_mut(),
                this.call.as_mut(),
                this.judge,
                cx
            ));
            let savepoint = this.attempt.take_savepoint();
            let attempted = match ended {
                Ok(attempted) => attempted,
                Err(unwound) => match savepoint {
                    Some(savepoint) => {
                        *this.settling = Some(savepoint.settle(false, Ended::Unwound(unwound)));
                        continue;
                    }
                    None => std::panic::resume_unwind(unwound),
                },
            };
            let attempted = run.finish(attempted);
            let Some(savepoint) = savepoint else {
                break settled(attempted);
            };
            let keep = matches!(attempted, Ok((Ok(_), _)));
            *this.settling = Some(savepoint.settle(keep, Ended::Answer(attempted)));
        };
        *this.run = None;
        *this.stage = RunStage::Answered;
        released(this.call, this.wait);
        Poll::Ready(R::answered(ran))
    }
}

pin_project_lite::pin_project! {
    /// What [`ResiliencePolicy::run_again`] answers: a slot of the bulkhead
    /// taken, then each attempt of the run, each retry after its backoff.
    struct RunAgain<'p, I, E, B, R, F, Fut, J>
    where
        R: ResilientAnswer,
    {
        stage: RunStage,
        // In `RunOnce`'s order.
        #[pin]
        call: Again<F, Fut>,
        judge: J,
        attempt: Attempt<'p>,
        settling: Option<Box<Settling<Attempted<Result<R::Ok, R::Err>>>>>,
        #[pin]
        wait: Wait<'p, I, B>,
        run: Option<Run<'p, I>>,
        policy: &'p ResiliencePolicy<I>,
        method: Option<ResilientMethod>,
        enter: E,
        answer: PhantomData<fn() -> R>,
    }
}

impl<'p, I, E, B, R, F, Fut, J> Future for RunAgain<'p, I, E, B, R, F, Fut, J>
where
    I: Instance,
    E: Fn(&'p ResiliencePolicy<I>, &'p Bulkhead, Option<ResilientMethod>, Option<Instant>) -> B,
    B: Future<Output = Result<Run<'p, I>, Refusal>>,
    R: ResilientAnswer,
    F: FnMut(CancellationToken) -> Fut,
    Fut: Future<Output = R::Attempt>,
    J: Fn(&Result<R::Ok, R::Err>) -> Verdict,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<R> {
        let mut this = self.project();
        if let RunStage::Opening = *this.stage {
            let opened = ready!(poll_open(
                this.policy,
                *this.method,
                this.enter,
                this.wait.as_mut(),
                cx
            ));
            match opened {
                Ok(run) => {
                    *this.run = Some(run.in_savepoints(true));
                    *this.stage = RunStage::Attempting;
                }
                Err(refusal) => {
                    *this.stage = RunStage::Answered;
                    released(this.call, this.wait);
                    return Poll::Ready(R::answered(Err(refusal.into())));
                }
            }
        }
        let (RunStage::Attempting, Some(run)) = (*this.stage, this.run.as_mut()) else {
            unreachable!("a run is polled after it answered");
        };
        let ran = loop {
            if let Some(settling) = this.settling.as_mut() {
                let (ended, settled_as) = ready!(settling.poll(cx));
                *this.settling = None;
                match run.terms.after_savepoint(ended, settled_as) {
                    Ok(Next::Answer(attempted)) => break settled(attempted),
                    Ok(Next::Again(retry)) => run.retrying(retry.wait, retry.error.as_deref()),
                    Err(unsettled) => break Err(unsettled),
                }
            }
            if let WaitProj::BackingOff { backoff } = this.wait.as_mut().project() {
                ready!(backoff.poll(cx));
                this.wait.set(Wait::within());
                run.resent = run.resent.saturating_add(1);
                *this.attempt = Attempt::new();
            }
            let ended = ready!(this.attempt.poll_ended::<_, _, _, R, J>(
                run,
                this.wait.as_mut(),
                this.call.as_mut(),
                this.judge,
                cx
            ));
            let savepoint = this.attempt.take_savepoint();
            let mut attempted = match ended {
                Ok(attempted) => attempted,
                Err(unwound) => match savepoint {
                    Some(savepoint) => {
                        *this.settling = Some(savepoint.settle(false, Ended::Unwound(unwound)));
                        continue;
                    }
                    None => std::panic::resume_unwind(unwound),
                },
            };
            let retry = run.retry(&mut attempted);
            if let Some(retry) = &retry {
                // Armed as the retry is decided: the backoff counts from the
                // attempt's answer, its rollback inside it.
                this.wait.set(Wait::BackingOff {
                    backoff: tokio::time::sleep(retry.wait),
                });
            }
            match (savepoint, retry) {
                (Some(savepoint), Some(retry)) => {
                    *this.settling = Some(savepoint.settle(false, Ended::Again(retry)));
                }
                (Some(savepoint), None) => {
                    let attempted = run.finish(attempted);
                    let keep = matches!(attempted, Ok((Ok(_), _)));
                    *this.settling = Some(savepoint.settle(keep, Ended::Answer(attempted)));
                }
                (None, Some(retry)) => run.retrying(retry.wait, retry.error.as_deref()),
                (None, None) => break settled(run.finish(attempted)),
            }
        };
        *this.run = None;
        *this.stage = RunStage::Answered;
        released(this.call, this.wait);
        Poll::Ready(R::answered(ran))
    }
}

/// What a run holds let go as it answers, though its future is kept: the
/// closure, with what it captured, and the attempt's bound, whose timer would
/// wake the task once more.
fn released<I, B, C: Call>(call: Pin<&mut C>, mut wait: Pin<&mut Wait<'_, I, B>>) {
    call.release();
    wait.set(Wait::within());
}

/// A run of `method` opened under `policy` through
/// [`ResiliencePolicy::open`], its wait for a slot of the bulkhead, through
/// `enter`, built only when none is free and held in `wait`.
fn poll_open<'p, I, E, B>(
    policy: &'p ResiliencePolicy<I>,
    method: Option<ResilientMethod>,
    enter: &E,
    mut wait: Pin<&mut Wait<'p, I, B>>,
    cx: &mut Context<'_>,
) -> Poll<Result<Run<'p, I>, Refusal>>
where
    I: Instance,
    E: Fn(&'p ResiliencePolicy<I>, &'p Bulkhead, Option<ResilientMethod>, Option<Instant>) -> B,
    B: Future<Output = Result<Run<'p, I>, Refusal>>,
{
    if let WaitProj::Within { .. } = wait.as_mut().project() {
        match policy.open(method) {
            Opened::Run(run) => return Poll::Ready(Ok(run)),
            Opened::Refused(refusal) => return Poll::Ready(Err(refusal)),
            Opened::Full(bulkhead, deadline) => wait.set(Wait::Entering {
                entering: enter(policy, bulkhead, method, deadline),
            }),
        }
    }
    let WaitProj::Entering { entering } = wait.as_mut().project() else {
        unreachable!("a run waits for the slot it found none free of");
    };
    let entered = ready!(entering.poll(cx));
    wait.set(Wait::within());
    Poll::Ready(entered)
}

impl<I: Instance> nest_rs_core::CarriesRemedy for ResiliencePolicy<I> {
    fn remedy() -> Cow<'static, str> {
        let import =
            nest_rs_core::__private::typed_import::<I>("nest_rs::resilience::ResilienceModule");
        let variables =
            nest_rs_config::var_name(&instance_namespace::<I>(ResilienceConfig::NAMESPACE), "*");
        format!("Import `{import}`, which builds the policy from {variables}.").into()
    }
}

impl<I: Instance> fmt::Debug for ResiliencePolicy<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResiliencePolicy")
            .field("instance", &I::NAME)
            .field("timeout", &self.timeout)
            .field("retries", &self.retries)
            .field("breaker", &self.breaker.is_some())
            .field("bulkhead", &self.bulkhead.is_some())
            .field("rate", &self.rate.is_some())
            .finish_non_exhaustive()
    }
}

/// The line a call the rate refuses files, naming `method` when a decorated
/// method ran it, `wait` the time to the next token free.
fn refused_by_the_rate(instance: &'static str, method: Option<&ResilientMethod>, wait: Duration) {
    tracing::debug!(
        target: TARGET,
        instance,
        host = method.map(ResilientMethod::host).as_deref(),
        method = method.map(ResilientMethod::name),
        retry_after_ms = millis(wait),
        "{REFUSED_LIMITED}",
    );
}

/// How the answer of a call sent once counts: an `Ok` is a success; an `Err`
/// `judging` records is the dependency failing, said as the policy tells an
/// error ([`told`]), and any other `Err` counts in neither direction, and
/// says nothing.
fn judged_once<R, J>(judging: &J, answer: &Result<R::Ok, R::Err>) -> Verdict
where
    R: ResilientAnswer,
    R::Err: Error + 'static,
    J: Judge<R>,
{
    match answer {
        Ok(_) => Verdict::Succeeded,
        Err(error) if judging.records(error) => Verdict::Final {
            error: told(error).into(),
        },
        Err(_) => Verdict::Ignored,
    }
}

/// How one answer of a call a run may send again counts, on `judging`'s two
/// axes: an `Ok` is a success, sent again when it retries it; an `Err` is a
/// failure when it records it and counts in neither direction otherwise, and
/// is sent again when it retries it, after the wait a [`Problem`] in its
/// chain asks, whichever it counts for. Its chain is said for an `Err` that
/// is a failure or sent again, and for no other.
fn judged_again<R, J>(judging: &J, answer: &Result<R::Ok, R::Err>) -> Verdict
where
    R: ResilientAnswer,
    R::Err: Error + 'static,
    J: Judge<R>,
{
    let error = match answer {
        Ok(answer) if judging.retries_ok(answer) => return Verdict::Again,
        Ok(_) => return Verdict::Succeeded,
        Err(error) => error,
    };
    match (judging.records(error), judging.retries(error)) {
        (true, true) => Verdict::Failed {
            asked: asked(error),
            error: told(error).into(),
        },
        (true, false) => Verdict::Final {
            error: told(error).into(),
        },
        (false, true) => Verdict::IgnoredAgain {
            asked: asked(error),
            error: told(error).into(),
        },
        (false, false) => Verdict::Ignored,
    }
}

/// The wait a [`Problem`] in `error`'s chain asks before a retry.
fn asked(error: &(dyn Error + 'static)) -> Option<Duration> {
    nest_rs_core::__private::find_in_chain::<Problem>(error)
        .and_then(Problem::retry_after)
        .map(|seconds| Duration::from_secs(seconds.into()))
}

/// What an attempt a run sends again, when it may, answered in error — none,
/// for an `Ok` — and the wait its dependency asked before the retry: a timeout
/// or an answer judged one to send again, never one the run ends on, nor a
/// refusal that sent nothing — a unit out of time's included, which is no
/// attempt given up at its bound.
fn sent_again<T>(attempted: &Attempted<T>) -> Option<(Option<Duration>, Option<&str>)> {
    match attempted {
        Ok((_, Verdict::Failed { asked, error } | Verdict::IgnoredAgain { asked, error })) => {
            Some((*asked, Some(error)))
        }
        Ok((_, Verdict::Again)) => Some((None, None)),
        Ok((_, Verdict::Succeeded | Verdict::Final { .. } | Verdict::Ignored))
        | Err(Refusal::OUT_OF_TIME | Refusal::Open(_) | Refusal::Full | Refusal::Limited(_)) => {
            None
        }
        Err(Refusal::TimedOut(_)) => Some((None, Some(TIMED_OUT))),
    }
}

fn settled<T, E>(attempted: Attempted<Result<T, E>>) -> Result<T, ResilienceError<E>> {
    match attempted {
        Ok((Ok(value), _)) => Ok(value),
        Ok((Err(error), _)) => Err(ResilienceError::Failed(error)),
        Err(refusal) => Err(refusal.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU32;
    use std::task::Waker;

    use nest_rs_config::{Config, ConfigService};
    use nest_rs_core::{CarriesRemedy, error_message, instance};

    use super::*;
    use crate::resilient::{every, judging, none};

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
        Verdict::Failed {
            asked,
            error: error_message(&Refused).into(),
        }
    }

    /// How the answer of a call sent once counts with no predicate of its
    /// own: every `Err` a failure.
    fn judged<T, E: Error + 'static>(answer: &Result<T, E>) -> Verdict {
        judged_once::<Answer<T, E>, _>(&judging_by_default(), answer)
    }

    /// What a call answering `Result<T, E>` answers under its policy.
    type Answer<T, E> = Result<T, ResilienceError<E>>;

    /// Whether `run` attempts its call again after `attempted`, its backoff
    /// waited when so.
    async fn again<T>(run: &mut Run<'_, Upstream>, attempted: Attempted<T>) -> bool {
        match run.again(attempted) {
            Ok(backoff) => {
                backoff.await;
                true
            }
            Err(_ended) => false,
        }
    }

    /// Retries on, no breaker, no bulkhead: the subject is the retry.
    fn retrying(retries: usize) -> ResiliencePolicy<Upstream> {
        policy(ResilienceConfig {
            retries: Some(retries),
            breaker_window: None,
            ..ResilienceConfig::default()
        })
    }

    /// The two axes are independent: `record_if` alone decides whether an
    /// `Err` is a failure or counts in neither direction, `retry_if` and
    /// `retry_ok_if` alone the retry, and the wait a `Problem` in the chain
    /// asks rides an `Err` sent again whichever it counts for. The chain is
    /// told for an `Err` that is a failure or sent again, and for no other.
    #[test]
    fn an_answer_is_judged_on_two_axes_each_by_its_own_predicate() {
        let throttled =
            Throttled(Problem::new(429, nest_rs_core::code::RATE_LIMITED).with_retry_after(7));
        let chain: Cow<'static, str> = error_message(&throttled).into();
        type Quoted = Answer<u32, Throttled>;

        let asked = Some(Duration::from_secs(7));
        let err = Err::<u32, _>(throttled);
        let ok = Ok::<u32, Throttled>(3);
        let again = |retry_if: fn(&Throttled) -> bool,
                     retry_ok_if: fn(&u32) -> bool,
                     record_if: fn(&Throttled) -> bool,
                     answer: &Result<u32, Throttled>| {
            let judging = judging(PhantomData::<Quoted>, retry_if, retry_ok_if, record_if);
            judged_again::<Quoted, _>(&judging, answer)
        };
        let once = |record_if: fn(&Throttled) -> bool, answer: &Result<u32, Throttled>| {
            let judging = judging(PhantomData::<Quoted>, every, none, record_if);
            judged_once::<Quoted, _>(&judging, answer)
        };

        assert_eq!(
            again(every, none, every, &err),
            Verdict::Failed {
                asked,
                error: chain.clone(),
            },
            "by default an `Err` is a failure, sent again"
        );
        assert_eq!(
            again(none, none, every, &err),
            Verdict::Final {
                error: chain.clone(),
            },
            "a failure `retry_if` refuses ends the run"
        );
        assert_eq!(
            again(every, none, none, &err),
            Verdict::IgnoredAgain {
                asked,
                error: chain.clone(),
            },
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
            Verdict::Final { error: chain },
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

    #[tokio::test(start_paused = true)]
    async fn a_call_answering_at_once_is_its_own_answer() {
        let answered = retrying(2)
            .run(|_| async { Ok::<_, Refused>("charged") })
            .await;
        assert!(matches!(answered, Ok("charged")), "{answered:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_not_declared_idempotent_is_never_attempted_again() {
        let calls = AtomicU32::new(0);
        let answered = retrying(2)
            .run(|_| async {
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
            .run_idempotent(|_| async {
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
            .run_idempotent(|_| async {
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
        .run(|cancel| async move {
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
                Err(ResilienceError::TimedOut { timeout, .. }) if timeout == Duration::from_millis(300)
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
        let making = |_: CancellationToken| -> std::future::Ready<Result<(), Refused>> {
            panic!("the upstream's client panicked before its future")
        };
        let running = |_: CancellationToken| async {
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
            .run_idempotent(|cancel: CancellationToken| {
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
            .run(|cancel: CancellationToken| {
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
            .run_idempotent(|_| {
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
            matches!(answered, Err(ResilienceError::TimedOut { .. })),
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
            let moving = move |_| {
                std::hint::black_box(&buffer);
                SaysItsDrop(says)
            };
            assert!(size_of_val(&moving) > 16 * 1024, "past the threshold");
            let mut run: Says<'_> = if past {
                Box::pin(policy.run(moving))
            } else {
                Box::pin(policy.run(|_| SaysItsDrop(says)))
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
            let call = move |_| {
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
            let call = |_| async {
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
        .run_idempotent(|_| async {
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
            policy.run(|_| std::future::pending::<Result<(), Refused>>()),
        )
        .await;
        assert!(
            matches!(given_up, Err(ResilienceError::TimedOut { timeout, .. }) if timeout == left),
            "{given_up:?}"
        );
        assert_eq!(started.elapsed(), left, "the deadline, not the 5 s timeout");
        let refused = policy.run(|_| async { Ok::<_, Refused>(()) }).await;
        assert!(
            matches!(refused, Err(ResilienceError::Open { .. })),
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
            policy.run_idempotent(|_| failing()),
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
            policy.run_idempotent(|_| failing()),
        )
        .await;
        assert!(
            matches!(spent, Err(ResilienceError::TimedOut { timeout, .. }) if timeout.is_zero()),
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
            policy.run_idempotent(|_| async {
                calls.fetch_add(1, Ordering::Relaxed);
                Err::<(), _>(Refused)
            }),
        ));
        park(&mut failing).await;
        tokio::time::advance(Duration::from_secs(2)).await;
        let late = failing.await;
        assert!(
            matches!(late, Err(ResilienceError::TimedOut { timeout, .. }) if timeout.is_zero()),
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
        let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        let spent = nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now()),
            policy.run(|_| answering()),
        )
        .await;
        assert!(
            matches!(spent, Err(ResilienceError::TimedOut { timeout, .. }) if timeout.is_zero()),
            "{spent:?}"
        );
        let open = policy.run(|_| answering()).await;
        assert!(
            matches!(open, Err(ResilienceError::Open { .. })),
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
            .run_idempotent(|_| async {
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
            .run_idempotent(|_| async {
                calls.fetch_add(1, Ordering::Relaxed);
                Err::<(), _>(Refused)
            })
            .await;
        assert!(
            matches!(answered, Err(ResilienceError::Open { .. })),
            "the breaker opened under the run: {answered:?}"
        );
        assert_eq!(
            calls.load(Ordering::Relaxed),
            2,
            "no attempt past the one that opened it"
        );

        let refused = policy
            .run(|_| async {
                calls.fetch_add(1, Ordering::Relaxed);
                Ok::<_, Refused>(())
            })
            .await;
        let Err(ResilienceError::Open { retry_after, .. }) = refused else {
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
                    .run(|_| async move {
                        release.notified().await;
                        Ok::<_, Refused>(())
                    })
                    .await
            })
        };
        tokio::task::yield_now().await;
        let refused = policy.run(|_| async { Ok::<_, Refused>(()) }).await;
        assert!(
            matches!(refused, Err(ResilienceError::Full { .. })),
            "{refused:?}"
        );
        release.notify_one();
        assert!(holding.await.expect("the held call ends").is_ok());
        assert!(policy.run(|_| async { Ok::<_, Refused>(()) }).await.is_ok());
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
        let held = policy
            .begin(None)
            .await
            .expect("the one slot, while closed");
        policy
            .breaker
            .as_ref()
            .expect("a breaker")
            .admit(policy.timeout)
            .expect("closed")
            .send(policy.timeout, None)
            .settle(Some(TIMED_OUT));

        let started = tokio::time::Instant::now();
        let refused = policy.run(|_| async { Ok::<_, Refused>(()) }).await;
        assert!(
            matches!(refused, Err(ResilienceError::Open { .. })),
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
                policy.run(|_| answering()),
            )
        };
        let held = policy.begin(None).await.expect("the one slot");
        let refused = spent().await;
        assert!(
            matches!(refused, Err(ResilienceError::TimedOut { timeout, .. }) if timeout.is_zero()),
            "out of time before the full bulkhead: {refused:?}"
        );
        drop(held);

        let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        let refused = spent().await;
        assert!(
            matches!(refused, Err(ResilienceError::TimedOut { timeout, .. }) if timeout.is_zero()),
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
        let held = policy.begin(None).await.expect("the one slot");
        let sent = AtomicU32::new(0);
        let mut waiting = Box::pin(nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now() + Duration::from_secs(5)),
            policy.run(|_| async {
                sent.fetch_add(1, Ordering::Relaxed);
                Ok::<_, Refused>(())
            }),
        ));
        park(&mut waiting).await;
        drop(held);
        tokio::time::advance(Duration::from_secs(6)).await;
        let late = waiting.await;
        assert!(
            matches!(late, Err(ResilienceError::TimedOut { timeout, .. }) if timeout.is_zero()),
            "the slot freed for it, its task run at 6 s, past its deadline at 5 s: {late:?}"
        );
        assert_eq!(
            sent.load(Ordering::Relaxed),
            0,
            "a unit out of time sends nothing"
        );
        assert!(
            policy.run(|_| answering()).await.is_ok(),
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
                        .run(|_| async {
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
                (at, Err(ResilienceError::Full { .. })) => refused.push(at),
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
        let tracking =
            crate::resilient::resilient_method(|| "ShipmentsService", "track", None, None);
        let held = policy.begin(None).await.expect("the one slot");
        let started = tokio::time::Instant::now();
        let (waited, crowded) = tokio::join!(
            policy.run_once::<Ran, _, _, _>(Some(tracking), judging_by_default(), |_| answering()),
            async {
                tokio::task::yield_now().await;
                let crowded = policy
                    .run_once::<Ran, _, _, _>(Some(tracking), judging_by_default(), |_| answering())
                    .await;
                (started.elapsed(), crowded)
            },
        );
        drop(held);
        assert!(
            matches!(waited, Err(ResilienceError::Full { .. })),
            "its wait ran out: {waited:?}"
        );
        assert!(
            matches!(crowded, (at, Err(ResilienceError::Full { .. })) if at.is_zero()),
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

    /// A wait and a bound on the waiters mean nothing without slots, as a
    /// window and a wait mean nothing without a rate limit: the policy builds
    /// no bulkhead, and sends every call at once.
    #[tokio::test(start_paused = true)]
    async fn a_policy_with_no_slots_builds_no_bulkhead_whatever_its_wait_and_waiters() {
        let policy = Arc::new(policy(ResilienceConfig {
            retries: None,
            breaker_window: None,
            concurrency: None,
            concurrency_wait: Some(Duration::from_secs(60)),
            concurrency_waiters: Some(1),
            ..ResilienceConfig::default()
        }));
        assert!(policy.bulkhead.is_none());
        assert!(
            format!("{policy:?}").contains("bulkhead: false"),
            "{policy:?}"
        );
        let started = tokio::time::Instant::now();
        let calls: Vec<_> = (0..10)
            .map(|_| {
                let policy = Arc::clone(&policy);
                tokio::spawn(async move {
                    policy
                        .run(|_| async {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            Ok::<_, Refused>(())
                        })
                        .await
                })
            })
            .collect();
        for call in calls {
            let answered = call.await.expect("the call's task");
            assert!(answered.is_ok(), "{answered:?}");
        }
        assert_eq!(started.elapsed(), Duration::from_secs(1), "all at once");
    }

    #[tokio::test(start_paused = true)]
    async fn an_asked_wait_is_honoured_up_to_the_ceiling_and_past_it_ends_the_run() {
        let policy = retrying(3);
        let mut run = policy.begin(None).await.expect("a run");
        let asked = Duration::from_secs(1);
        let started = tokio::time::Instant::now();
        let failed: Attempted<()> = Ok(((), failing(Some(asked))));
        assert!(again(&mut run, failed).await);
        let waited = started.elapsed();
        assert!(
            waited >= asked && waited <= asked.mul_f64(1.2),
            "the ask, stretched by a fifth at most: {waited:?}"
        );
        let too_long: Attempted<()> = Ok(((), failing(Some(Duration::from_secs(3)))));
        assert!(!again(&mut run, too_long).await, "past the 2 s ceiling");
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
            .find(|seed| policy.backoff.wait(NonZeroU32::MIN, *seed) < floor)
            .expect("a seed drawing the jitter's lower half");
        let drawn = policy.backoff.wait(NonZeroU32::MIN, seed);
        let run = || async {
            let mut run = policy.begin(None).await.expect("a run");
            run.seed = Some(seed);
            run
        };
        let failed: Attempted<()> = Ok(((), failing(None)));

        let mut retrying = run().await;
        let started = tokio::time::Instant::now();
        assert!(
            again(&mut retrying, failed.clone()).await,
            "attempted again"
        );
        assert!(
            started.elapsed() >= drawn,
            "{:?}, drawn {drawn:?}",
            started.elapsed()
        );

        let ended = nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now() + drawn / 2),
            async {
                let mut run = run().await;
                again(&mut run, failed).await
            },
        )
        .await;
        assert!(!ended, "the unit's deadline comes before the wait ends");

        let asked: Attempted<()> = Ok(((), failing(Some(Duration::from_secs(3)))));
        let mut asking = run().await;
        assert!(!again(&mut asking, asked).await, "past the ceiling");

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
        assert!(policy.run(|_| sent()).await.is_ok());
        let retried = policy
            .run_idempotent(|_| async {
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
        let refused = policy.run(|_| sent()).await;
        assert!(
            matches!(refused, Err(ResilienceError::Limited { .. })),
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
        assert!(policy.run(|_| sent()).await.is_ok());
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(policy.run(|_| sent()).await.is_ok());
        let started = tokio::time::Instant::now();
        let Err(refused) = policy.run(|_| sent()).await else {
            panic!("past the limit");
        };
        let ResilienceError::Limited { retry_after, .. } = &refused else {
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
                policy.run(|_| answering()),
            )
        };
        let refused = spent().await;
        assert!(
            matches!(refused, Err(ResilienceError::TimedOut { timeout, .. }) if timeout.is_zero()),
            "{refused:?}"
        );
        assert!(
            policy.run(|_| answering()).await.is_ok(),
            "the attempt out of time took no token"
        );
        let refused = spent().await;
        assert!(
            matches!(refused, Err(ResilienceError::TimedOut { timeout, .. }) if timeout.is_zero()),
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
        let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        for _ in 0..3 {
            let refused = policy.run(|_| answering()).await;
            assert!(
                matches!(refused, Err(ResilienceError::Open { .. })),
                "{refused:?}"
            );
        }
        tokio::time::advance(Duration::from_secs(10)).await;
        let probed = policy.run(|_| answering()).await;
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
        assert!(policy.run(|_| answering()).await.is_ok());
        let limited = policy.run(|_| answering()).await;
        assert!(
            matches!(limited, Err(ResilienceError::Limited { .. })),
            "{limited:?}"
        );
        tokio::time::advance(Duration::from_secs(60)).await;
        let answered = policy.run(|_| answering()).await;
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
            let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
            assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        }
        tokio::time::advance(Duration::from_secs(30)).await;
        for nth in ["the probe", "the call after it, probing in its turn"] {
            let refused = policy.run(|_| answering()).await;
            assert!(
                matches!(refused, Err(ResilienceError::Limited { retry_after, .. })
                    if retry_after == Duration::from_secs(30)),
                "{nth}, admitted by the breaker and refused by the rate: {refused:?}"
            );
        }
        tokio::time::advance(Duration::from_secs(30)).await;
        let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
        assert!(
            matches!(failed, Err(ResilienceError::Failed(Refused))),
            "a token is back, and a probe is sent at last: {failed:?}"
        );
        let reopened = policy.run(|_| answering()).await;
        assert!(
            matches!(reopened, Err(ResilienceError::Open { .. })),
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
        assert!(policy.run(|_| sent()).await.is_ok());
        tokio::time::advance(Duration::from_secs(4)).await;
        let started = tokio::time::Instant::now();
        let answered = policy.run(|_| sent()).await;
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
        assert!(policy.run(|_| answering()).await.is_ok());
        tokio::time::advance(Duration::from_secs(4)).await;
        let started = tokio::time::Instant::now();
        let refused = policy.run(|_| answering()).await;
        assert!(
            matches!(refused, Err(ResilienceError::Limited { retry_after, .. })
                if retry_after == Duration::from_secs(6)),
            "a token 6 s away, past the 5 s wait: {refused:?}"
        );
        assert_eq!(started.elapsed(), Duration::ZERO, "nothing waited");

        tokio::time::advance(Duration::from_secs(2)).await;
        let deadline = |left| Some(tokio::time::Instant::now() + left);
        let refused = nest_rs_core::__private::with_deadline(
            deadline(Duration::from_secs(4)),
            policy.run(|_| answering()),
        )
        .await;
        assert!(
            matches!(refused, Err(ResilienceError::Limited { retry_after, .. })
                if retry_after == Duration::from_secs(4)),
            "a token at the unit's deadline leaves its call no time: {refused:?}"
        );
        assert_eq!(started.elapsed(), Duration::from_secs(2), "nothing waited");

        let answered = nest_rs_core::__private::with_deadline(
            deadline(Duration::from_secs(5)),
            policy.run(|_| answering()),
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
            assert!(policy.run(|_| answering()).await.is_ok());
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
            .run_idempotent(|_| async {
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
        let sending = |_| async {
            sent.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Refused>(())
        };
        let succeeded = |_: &Result<(), Refused>| Verdict::Succeeded;
        let started = tokio::time::Instant::now();
        for at in [Duration::ZERO, window] {
            let run = policy.begin(None).await.expect("a run");
            let (attempted, _outstanding) = run.attempt_once(sending, succeeded).await;
            assert!(matches!(attempted, Ok((Ok(()), Verdict::Succeeded))));
            assert_eq!(started.elapsed(), at, "the second waited for its token");
        }
        let first = policy.begin(None).await.expect("a run");
        let second = policy.begin(None).await.expect("a run");
        let ((waited, _), (refused, _)) = tokio::join!(
            first.attempt_once(sending, succeeded),
            second.attempt_once(sending, succeeded),
        );
        assert!(matches!(waited, Ok((Ok(()), Verdict::Succeeded))));
        assert!(
            matches!(refused, Err(Refusal::Limited(wait)) if wait == window * 2),
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
            assert!(policy.run(|_| answering()).await.is_ok());
            let started = tokio::time::Instant::now();
            let answered = |answered: Ran| match answered {
                Ok(()) => format!("sent at {:?}", started.elapsed()),
                Err(ResilienceError::Limited { retry_after, .. }) => {
                    format!("limited {retry_after:?} at {:?}", started.elapsed())
                }
                Err(other) => panic!("{other:?}"),
            };
            let (first, second) = tokio::join!(
                async { answered(policy.run(|_| answering()).await) },
                async { answered(policy.run(|_| answering()).await) },
            );
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
        assert!(policy.run(|_| answering()).await.is_ok());
        let started = tokio::time::Instant::now();
        let mut waiting = Box::pin(policy.run(|_| answering()));
        park(&mut waiting).await;
        tokio::time::advance(Duration::from_secs(12)).await;
        let mut arrived = Box::pin(policy.run(|_| answering()));
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
        assert!(policy.run(|_| answering()).await.is_ok());
        let started = tokio::time::Instant::now();
        let mut first = Box::pin(policy.run(|_| answering()));
        park(&mut first).await;
        let behind = tokio::spawn({
            let policy = Arc::clone(&policy);
            async move {
                let answered = policy.run(|_| answering()).await;
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
            let rate = policy.rate.as_ref().expect("a rate limit");
            assert!(
                matches!(rate.arrive(started + later, None), Arrived::Taken),
                "the other call takes the token"
            );
            let Err(refused) = policy.run(|_| answering()).await else {
                panic!("with {wait:?} to wait, no token is free");
            };
            assert!(
                matches!(refused, ResilienceError::Limited { retry_after, .. }
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
        assert!(policy.run(|_| answering()).await.is_ok());
        let (waited, refused) = tokio::join!(policy.run(|_| answering()), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            policy.run(|_| answering()).await
        });
        assert!(matches!(waited, Ok(())), "{waited:?}");
        assert!(
            matches!(refused, Err(ResilienceError::Full { .. })),
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
        assert!(policy.run(|_| answering()).await.is_ok());
        let started = tokio::time::Instant::now();
        let (waited, behind) = tokio::join!(policy.run(|_| answering()), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let answered = policy.run(|_| answering()).await;
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
        let tracking =
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
                policy.run(|_| async {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    Ok::<_, Refused>(())
                }),
                async {
                    tokio::task::yield_now().await;
                    let waited = policy.run(|_| answering()).await;
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

            let held = policy.begin(None).await.expect("the slot, free again");
            let refused_at = tokio::time::Instant::now();
            let refused = nest_rs_core::__private::with_deadline(
                left.map(|left| refused_at + left),
                policy.run_once::<Ran, _, _, _>(Some(tracking), judging_by_default(), |_| {
                    answering()
                }),
            )
            .await;
            assert!(
                matches!(refused, Err(ResilienceError::Limited { retry_after, .. })
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
        assert!(policy.run(|_| answering()).await.is_ok());
        let started = tokio::time::Instant::now();
        tokio::time::sleep(Duration::from_secs(6)).await;
        let (waited, behind) = tokio::join!(policy.run(|_| answering()), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let refused = policy.run(|_| answering()).await;
            (refused, started.elapsed())
        });
        assert!(waited.is_ok(), "{waited:?}");
        assert!(
            matches!(
                behind,
                (Err(ResilienceError::Limited { retry_after, .. }), at)
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
            policy.run(|_| async {
                tokio::time::sleep(Duration::from_secs(3)).await;
                Ok::<_, Refused>(())
            }),
            async {
                tokio::task::yield_now().await;
                let waited = policy.run(|_| answering()).await;
                (waited, started.elapsed())
            },
        );
        assert!(holding.is_ok(), "{holding:?}");
        assert!(
            matches!(
                waited,
                (Err(ResilienceError::Limited { retry_after, .. }), at)
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
            policy.run(|_| answering()).await.is_ok(),
            "the bucket's one token"
        );
        let held = policy.begin(None).await.expect("the one slot");
        let started = tokio::time::Instant::now();

        let spent =
            nest_rs_core::__private::with_deadline(Some(started), policy.run(|_| answering()))
                .await;
        assert!(
            matches!(spent, Err(ResilienceError::TimedOut { timeout, .. }) if timeout.is_zero()),
            "out of time, before the spent rate: {spent:?}"
        );
        policy
            .open_breaker(Duration::from_secs(30))
            .expect("an open time `breaker_open` takes");
        let open = policy.run(|_| answering()).await;
        assert!(
            matches!(open, Err(ResilienceError::Open { retry_after, .. })
                if retry_after == Duration::from_secs(30)),
            "the breaker opened by hand, before the spent rate: {open:?}"
        );
        assert!(
            lines.find(TARGET, REFUSED_LIMITED).is_empty(),
            "the rate refused neither"
        );

        policy.close_breaker().expect("a breaker");
        let limited = policy.run(|_| answering()).await;
        assert!(
            matches!(limited, Err(ResilienceError::Limited { retry_after, .. })
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
                policy.run(|_| async {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Ok::<_, Refused>(())
                }),
                async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let arrived = tokio::time::Instant::now();
                    let refused = policy.run(|_| answering()).await;
                    (arrived.elapsed(), refused)
                },
            );
            assert!(matches!(probed, Ok(())), "the probe {probe}: {probed:?}");
            assert!(
                matches!(refused, (waited, Err(ResilienceError::Open { retry_after, .. }))
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
        assert!(policy.run(|_| answering()).await.is_ok());
        let started = tokio::time::Instant::now();
        let answered = policy.run(|_| slow()).await;
        assert!(
            matches!(answered, Ok("answered")),
            "a 5 s wait, then 800 ms of its 1 s timeout: {answered:?}"
        );
        assert_eq!(started.elapsed(), Duration::from_millis(5_800));

        let started = tokio::time::Instant::now();
        let cut = nest_rs_core::__private::with_deadline(
            Some(started + Duration::from_millis(4_700)),
            policy.run(|_| slow()),
        )
        .await;
        assert!(
            matches!(cut, Err(ResilienceError::TimedOut { timeout, .. })
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
        assert!(policy.run(|_| answering()).await.is_ok());
        let sent = AtomicU32::new(0);
        let mut waiting = Box::pin(nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now() + Duration::from_secs(11)),
            policy.run(|_| async {
                sent.fetch_add(1, Ordering::Relaxed);
                std::future::pending::<Result<(), Refused>>().await
            }),
        ));
        park(&mut waiting).await;
        tokio::time::advance(Duration::from_secs(12)).await;
        let late = waiting.await;
        assert!(
            matches!(late, Err(ResilienceError::TimedOut { timeout, .. }) if timeout.is_zero()),
            "its wait for the token at 10 s ended at 12 s, past its deadline at 11 s: {late:?}"
        );
        assert_eq!(
            sent.load(Ordering::Relaxed),
            0,
            "a unit out of time sends nothing"
        );
        let resumed = tokio::time::Instant::now();
        assert!(policy.run(|_| answering()).await.is_ok());
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
            .run_idempotent(|_| async {
                calls.fetch_add(1, Ordering::Relaxed);
                Err::<(), _>(Refused)
            })
            .await;
        assert!(
            matches!(answered, Err(ResilienceError::Limited { .. })),
            "its first retry was refused: {answered:?}"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        let waited = started.elapsed();
        assert!(
            waited <= Duration::from_millis(240),
            "the one backoff before the retry the rate refused, and none after it: {waited:?}"
        );

        let mut run = policy.begin(None).await.expect("a run");
        let limited: Attempted<()> = Err(Refusal::Limited(Duration::from_secs(1)));
        assert!(!again(&mut run, limited).await);
    }

    /// A window and a wait mean nothing without a limit: the policy builds no
    /// rate, as a slot's wait builds no bulkhead without its slots, and sends
    /// every call with no lock taken and no timer armed for it.
    #[tokio::test(start_paused = true)]
    async fn a_policy_with_no_rate_limit_builds_no_rate_whatever_its_window_and_wait() {
        let policy = policy(ResilienceConfig {
            rate_limit: None,
            ..rated(1, Duration::from_secs(1), Some(Duration::from_secs(60)))
        });
        assert!(policy.rate.is_none());
        assert!(format!("{policy:?}").contains("rate: false"), "{policy:?}");
        let calls = AtomicU32::new(0);
        let started = tokio::time::Instant::now();
        for _ in 0..1_000 {
            let answered = policy
                .run(|_| async {
                    calls.fetch_add(1, Ordering::Relaxed);
                    Ok::<_, Refused>(())
                })
                .await;
            assert!(answered.is_ok(), "{answered:?}");
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1_000);
        assert_eq!(started.elapsed(), Duration::ZERO);
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
            let failed = policy.run(|_| failing()).await;
            assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        }
        tokio::time::advance(Duration::from_secs(30)).await;

        let mut waiting = Box::pin(policy.run(|_| failing()));
        park(&mut waiting).await;
        drop(waiting);

        let started = tokio::time::Instant::now();
        let probed = policy.run(|_| failing()).await;
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
        let reopened = policy.run(|_| answering()).await;
        assert!(
            matches!(reopened, Err(ResilienceError::Open { .. })),
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
        let mut holding = Box::pin(policy.run_idempotent(|_| async {
            if calls.fetch_add(1, Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
                return Err(Refused);
            }
            std::future::pending::<Result<(), Refused>>().await
        }));
        park(&mut holding).await;
        let waiting = PollsOnWake::new(policy.run(|_| async { Ok::<_, Refused>("sent") }));
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
        let mut holding = Box::pin(policy.run(|_| async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok::<_, Refused>("held")
        }));
        park(&mut holding).await;
        let mut probing = Box::pin(policy.run(|_| std::future::pending::<Result<&str, Refused>>()));
        park(&mut probing).await;
        let waiting = PollsOnWake::new(policy.run(|_| async { Ok::<_, Refused>("sent") }));
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
            let mut failing = Box::pin(policy.run(|_| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Err::<(), _>(Refused)
            }));
            park(&mut failing).await;
            let mut probing: Dropped<'_> = if again {
                Box::pin(policy.run_idempotent(|_| answering()))
            } else {
                Box::pin(policy.run(|_| answering()))
            };
            park(&mut probing).await;
            let waiting = PollsOnWake::new(policy.run(|_| async { Ok::<_, Refused>("sent") }));
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
        let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
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
        let answering = |_| async { Ok::<_, Refused>("answered") };

        opened_then_passed(&policy).await;
        let run = policy.begin(None).await.expect("the probe's run");
        let (attempted, outstanding) = run.attempt_once(answering, |_| failing(None)).await;
        assert!(matches!(
            attempted,
            Ok((Ok("answered"), Verdict::Failed { .. }))
        ));
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "a failure counts as it is judged, whatever is outstanding"
        );
        drop((outstanding, run));

        tokio::time::advance(Duration::from_secs(1)).await;
        let run = policy.begin(None).await.expect("the probe's run");
        let (attempted, outstanding) = run.attempt_once(answering, judged).await;
        assert!(matches!(
            attempted,
            Ok((Ok("answered"), Verdict::Succeeded))
        ));
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::HalfOpen),
            "the probe's success is not counted yet"
        );
        drop(outstanding);
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
        drop(run);

        opened_then_passed(&policy).await;
        let run = policy.begin(None).await.expect("the probe's run");
        let (_, outstanding) = run.attempt_once(answering, judged).await;
        let mut held = run.hold(outstanding, Arc::clone(&policy));
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
        let run = policy.begin(None).await.expect("the one slot");
        let next = Arc::clone(&policy);
        let waiting =
            PollsOnWake::new(async move { next.run(|_| async { Ok::<_, Refused>("sent") }).await });
        waiting.poll_now();
        assert!(
            waiting.answered().is_none(),
            "it waits for the slot the run holds"
        );
        let (failed, _) = run
            .attempt_once(|_| async { Err::<(), _>(Refused) }, judged)
            .await;
        assert!(matches!(failed, Ok((Err(Refused), Verdict::Final { .. }))));
        tokio::time::advance(Duration::from_secs(1)).await;
        let (_, outstanding) = run
            .attempt_once(|_| async { Ok::<_, Refused>(()) }, judged)
            .await;
        let held = run.hold(outstanding, Arc::clone(&policy));
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
        let run = policy.begin(None).await.expect("a run");
        let (given_up, outstanding) = run
            .attempt_once(|_| std::future::pending::<Result<(), Refused>>(), judged)
            .await;
        assert_eq!(
            given_up.err(),
            Some(Refusal::TimedOut(Duration::from_secs(2)))
        );
        assert_eq!(sent.elapsed(), Duration::from_secs(2));
        drop((outstanding, run));
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));

        let run = policy.begin(None).await.expect("a run");
        let panicking = run.attempt_once(
            |_| async {
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
            error_of(&lines, crate::breaker::MOVED).as_deref(),
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
        let at_the_bound = |_| async move {
            tokio::time::sleep(timeout).await;
            Ok::<_, Refused>("answered")
        };

        let sent = Instant::now();
        let run = policy.begin(None).await.expect("a run");
        let (attempted, outstanding) = run.attempt_once(at_the_bound, judged).await;
        assert_eq!(sent.elapsed(), timeout);
        assert!(
            matches!(attempted, Ok((Ok("answered"), Verdict::Succeeded))),
            "{attempted:?}"
        );
        drop((outstanding, run));
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
            let run = policy.begin(None).await.expect("a run");
            let answering = |_| async { Ok::<_, Refused>("answered") };
            let (attempted, _outstanding) = run.attempt_once(answering, judged).await;
            attempted
        });
        assert!(
            matches!(attempted, Ok((Ok("answered"), Verdict::Succeeded))),
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
                policy.run(|_| async { Ok::<_, Refused>("beside") }),
            ));
            let answered = std::cell::RefCell::new(None);
            let poll_beside = || {
                let mut cx = Context::from_waker(std::task::Waker::noop());
                if let Poll::Ready(answer) = beside.borrow_mut().as_mut().poll(&mut cx) {
                    *answered.borrow_mut() = Some(answer);
                }
            };
            let probe = |_: CancellationToken| async {
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
                matches!(refused, Some(Err(ResilienceError::Open { .. }))),
                "again: {again}; polled as the probe's future was torn down, it met the probe's \
                 turn still held: {refused:?}"
            );
            let next = policy.run(|_| async { Ok::<_, Refused>("next") }).await;
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
                Box::pin(policy.run_idempotent(probe))
            } else {
                Box::pin(policy.run(probe))
            };
            let parked =
                std::future::poll_fn(|cx| Poll::Ready(probing.as_mut().poll(cx).is_pending()))
                    .await;
            assert!(parked, "the probe's call never answers");
            assert_eq!(policy.breaker_state(), Some(BreakerState::HalfOpen));
            let token = handed.take().expect("the probe's call was made");
            let woken = PollsOnWake::new(policy.run(|_| async { Ok::<_, Refused>("woken") }));
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
                policy.run(|_| async {
                    tokio::time::advance(Duration::from_millis(3)).await;
                    std::future::pending::<Result<(), Refused>>().await
                }),
            ),
        )
        .await;
        assert!(
            matches!(cut, Ok(Err(ResilienceError::TimedOut { timeout, .. }))
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
                policy.run(|_| async {
                    tokio::time::advance(Duration::from_millis(3)).await;
                    std::future::pending::<Result<(), Refused>>().await
                }),
            ),
        )
        .await;
        assert!(
            matches!(cut, Ok(Err(ResilienceError::TimedOut { .. }))),
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
                policy.run_idempotent(|_| async {
                    calls.fetch_add(1, Ordering::Relaxed);
                    tokio::time::advance(Duration::from_millis(3)).await;
                    std::future::pending::<Result<(), Refused>>().await
                }),
            ),
        )
        .await;
        assert!(
            matches!(cut, Ok(Err(ResilienceError::TimedOut { timeout, .. }))
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
            let call = |_| std::future::pending::<Result<(), Refused>>();
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
            policy.run(|_| std::future::pending::<Result<(), Refused>>()),
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
            .find(TARGET, crate::breaker::MOVED)
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
        let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(30)).await;
        let (probed, refused) = tokio::join!(policy.run(|_| answering()), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            policy.run(|_| answering()).await
        });
        assert!(matches!(probed, Ok(())), "{probed:?}");
        assert!(
            matches!(refused, Err(ResilienceError::Open { retry_after, .. })
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
        let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
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
            policy.run(|_| async {
                tokio::time::sleep(Duration::from_secs(2)).await;
                Ok::<_, Refused>(())
            }),
            async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                policy.run(|_| answering()).await
            },
        );
        assert!(matches!(probed, Ok(())), "{probed:?}");
        assert!(
            matches!(refused, Err(ResilienceError::Open { retry_after, .. })
                if retry_after == Duration::from_secs(4)),
            "the probe's call sent at once, 1 s into its 5 s timeout: {refused:?}"
        );
    }

    /// A probe from a method pinning its own timeout is waited for as long.
    #[tokio::test(start_paused = true)]
    async fn a_call_refused_while_a_method_pinning_its_timeout_probes_is_told_what_is_left_of_it() {
        let policy = policy(probing(Duration::from_secs(5)));
        probe_due(&policy).await;
        let tracking =
            crate::resilient::resilient_method(|| "ShipmentsService", "track", Some(30_000), None);
        let (probed, refused) = tokio::join!(
            policy.run_once::<Ran, _, _, _>(Some(tracking), judging_by_default(), |_| async {
                tokio::time::sleep(Duration::from_secs(20)).await;
                Ok::<_, Refused>(())
            }),
            async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                policy.run(|_| answering()).await
            },
        );
        assert!(
            matches!(probed, Ok(())),
            "20 s of the method's 30 s: {probed:?}"
        );
        assert!(
            matches!(refused, Err(ResilienceError::Open { retry_after, .. })
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
                policy.run(|_| std::future::pending::<Result<(), Refused>>()),
            ),
            async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                policy.run(|_| answering()).await
            },
        );
        assert!(
            matches!(probed, Err(ResilienceError::TimedOut { timeout, .. })
                if timeout == Duration::from_secs(2)),
            "{probed:?}"
        );
        assert!(
            matches!(refused, Err(ResilienceError::Open { retry_after, .. })
                if retry_after == Duration::from_secs(1)),
            "1 s into the 2 s its unit had left, under the 5 s timeout: {refused:?}"
        );
    }

    /// A probe holding its turn while it waits for the rate's token is waited
    /// for until that token, then for its own bound: the timeout its method
    /// pinned, or what its unit's deadline leaves of the policy's.
    #[tokio::test(start_paused = true)]
    async fn a_probe_waiting_for_its_token_is_waited_for_until_it_then_for_its_own_bound() {
        let tracking =
            crate::resilient::resilient_method(|| "ShipmentsService", "track", Some(30_000), None);
        for (method, left, told, bound) in [
            (
                Some(tracking),
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
                    policy.run_once::<Ran, _, _, _>(method, judging_by_default(), |_| answering())
                ),
                async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    policy.run(|_| answering()).await
                },
            );
            assert!(matches!(probed, Ok(())), "sent at its token: {probed:?}");
            assert!(
                matches!(refused, Err(ResilienceError::Open { retry_after, .. })
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
        assert!(
            policy.run(|_| answering()).await.is_ok(),
            "one probe of two"
        );
        let cut = policy
            .run(|_| std::future::pending::<Result<(), Refused>>())
            .await;
        assert!(
            matches!(cut, Err(ResilienceError::TimedOut { timeout, .. })
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
        assert!(policy.run(|_| answering()).await.is_ok());
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::HalfOpen),
            "one probe of two since it opened again"
        );
        assert!(policy.run(|_| answering()).await.is_ok());
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
            policy.run(|_| async {
                sent.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_secs(1)).await;
                Err::<(), _>(Refused)
            }),
            async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                policy
                    .run(|_| async {
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
            matches!(waited, Err(ResilienceError::Open { retry_after, .. })
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
            move |_: CancellationToken| async move {
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
            matches!(beside, Err(ResilienceError::Open { retry_after, .. })
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
            policy.run(|_| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                Err::<(), _>(Refused)
            }),
            async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                policy.run(|_| answering()).await
            },
        );
        assert!(
            matches!(failed, Err(ResilienceError::Failed(Refused))),
            "{failed:?}"
        );
        assert!(matches!(probed, Ok(())), "{probed:?}");
        let states: Vec<String> = lines
            .find(TARGET, crate::breaker::MOVED)
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
                .find(TARGET, crate::breaker::MOVED)
                .iter()
                .map(|line| {
                    assert_eq!(line.level, "warn", "{line:?}");
                    line.field("state").unwrap_or_default()
                })
                .collect()
        };
        let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(1)).await;
        for _ in 0..10 {
            let refused = policy.run(|_| answering()).await;
            assert!(
                matches!(refused, Err(ResilienceError::Limited { .. })),
                "a token 59 s away, past the 30 s wait: {refused:?}"
            );
        }
        assert_eq!(states(), ["open"], "ten probes the rate refused, none sent");

        tokio::time::advance(Duration::from_secs(34)).await;
        let mut waiting = Box::pin(policy.run(|_| answering()));
        park(&mut waiting).await;
        drop(waiting);
        assert_eq!(
            states(),
            ["open"],
            "a probe dropped while it waited, never sent"
        );

        tokio::time::advance(Duration::from_secs(25)).await;
        assert!(policy.run(|_| answering()).await.is_ok());
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
                .find(TARGET, crate::breaker::MOVED)
                .iter()
                .map(|line| line.field("state").unwrap_or_default())
                .collect()
        };
        let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(1)).await;
        let mut probing = Box::pin(nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now() + Duration::from_secs(10)),
            policy.run(|_| answering()),
        ));
        park(&mut probing).await;
        tokio::time::advance(Duration::from_secs(11)).await;
        let late = probing.await;
        assert!(
            matches!(late, Err(ResilienceError::TimedOut { timeout, .. }) if timeout.is_zero()),
            "its wait for the token at 10 s ended at 12 s, past its deadline at 11 s: {late:?}"
        );
        assert_eq!(states(), ["open"], "the probe given up, never sent");

        let resumed = tokio::time::Instant::now();
        assert!(policy.run(|_| answering()).await.is_ok());
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
        assert!(policy.run(|_| answering()).await.is_ok());
        tokio::time::advance(Duration::from_secs(4)).await;
        let refused = policy.run(|_| answering()).await;
        assert!(
            matches!(refused, Err(ResilienceError::Limited { .. })),
            "{refused:?}"
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(policy.run(|_| answering()).await.is_ok());

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
            assert!(policy.run(|_| answering()).await.is_ok(), "the one token");
        }
        let near = Duration::from_micros(500);
        tokio::time::advance(window - near).await;
        let refused = refusing.run(|_| answering()).await;
        assert!(
            matches!(refused, Err(ResilienceError::Limited { retry_after, .. }) if retry_after == near),
            "{refused:?}"
        );
        let back = tokio::time::Instant::now() + near;
        let sent = std::cell::Cell::new(None);
        let answered = waiting
            .run(|_| async {
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
            .run_idempotent(|_| async { Err::<(), _>(unreachable()) })
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
            .run_idempotent(|_| std::future::pending::<Result<(), Refused>>())
            .await;
        assert!(
            matches!(timed_out, Err(ResilienceError::TimedOut { .. })),
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
            .run_idempotent(|_| async move { Err::<(), _>(asking()) })
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
            policy.run_idempotent(|_| async { Err::<(), _>(unreachable()) }),
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
            .run_again::<Polled, _, _, _>(
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
            .run_again::<Polled, _, _, _>(
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
    const BREAKER_MOVED: &str = crate::breaker::MOVED;

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
            let once = policy.run(|_| answering()).await;
            let again = policy.run_idempotent(|_| answering()).await;
            assert!(
                matches!(
                    (&once, &again),
                    (
                        Err(ResilienceError::Open { .. }),
                        Err(ResilienceError::Open { .. })
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
            .run_idempotent(|_| async { Err::<(), _>(Refused) })
            .await;
        assert!(
            matches!(opened, Err(ResilienceError::Open { .. })),
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
        let never = |_| std::future::pending::<Result<(), Refused>>();
        let timed_out = |ran: &Ran| matches!(ran, Err(ResilienceError::TimedOut { .. }));
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
                policy.run(|_| answering()),
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
                policy.run_idempotent(|_| async {
                    calls.fetch_add(1, Ordering::Relaxed);
                    Err::<(), _>(Refused)
                }),
            )
            .await;
            assert!(
                matches!(spent, Err(ResilienceError::TimedOut { timeout, .. }) if timeout.is_zero()),
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
            policy.run_again::<Polled, _, _, _>(
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
            let held = policy.begin(None).await.expect("the one slot");
            let lines = nest_rs_testing::LogCapture::install();
            let refused = policy.run(|_| answering()).await;
            assert!(
                matches!(refused, Err(ResilienceError::Full { .. })),
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
        assert!(policy.run(|_| answering()).await.is_ok());
        let lines = nest_rs_testing::LogCapture::install();
        let refused = policy.run(|_| answering()).await;
        assert!(
            matches!(refused, Err(ResilienceError::Limited { .. })),
            "{refused:?}"
        );
        assert_eq!(filed(&lines), [REFUSED_LIMITED]);
        drop(lines);

        let policy = self::policy(ResilienceConfig {
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(1)),
            ..rated(1, Duration::from_secs(60), None)
        });
        let held = policy.begin(None).await.expect("the one slot");
        let lines = nest_rs_testing::LogCapture::install();
        let taken = held.attempt_once(|_| answering(), judged).await;
        assert!(matches!(taken.0, Ok((Ok(()), _))), "the bucket's one token");
        let refused = policy.run(|_| answering()).await;
        assert!(
            matches!(refused, Err(ResilienceError::Limited { .. })),
            "{refused:?}"
        );
        assert_eq!(filed(&lines), [REFUSED_LIMITED]);
    }

    /// A unit of work whose transaction refuses to settle the savepoints it
    /// opens, each its own block.
    struct Unsettling;

    impl nest_rs_database::Executor for Unsettling {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn savepoint(&self) -> Option<Arc<dyn nest_rs_database::AtomicBlock>> {
            Some(Arc::new(Self))
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
        let failing = |_| async { Err::<(), _>(Refused) };
        let retrying = retrying(1);

        assert!(retrying.run_idempotent(failing).await.is_err());
        type Polled = Answer<u8, Refused>;
        let polled = retrying
            .run_again::<Polled, _, _, _>(
                None,
                judging(PhantomData::<Polled>, every, every, every),
                |_| async { Ok(40) },
            )
            .await;
        assert!(matches!(polled, Ok(40)), "{polled:?}");
        let asking = retrying
            .run_idempotent(|_| async {
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
            retrying.run_idempotent(|_| answering()),
        )
        .await;
        assert!(
            matches!(unsettled, Err(ResilienceError::Unsettled { .. })),
            "{unsettled:?}"
        );
        let never = retrying
            .run(|_| std::future::pending::<Result<(), Refused>>())
            .await;
        assert!(
            matches!(never, Err(ResilienceError::TimedOut { .. })),
            "{never:?}"
        );

        let rate = policy(rated(
            1,
            Duration::from_secs(10),
            Some(Duration::from_secs(5)),
        ));
        assert!(rate.run(|_| answering()).await.is_ok());
        tokio::time::advance(Duration::from_secs(4)).await;
        let limited = rate.run(|_| answering()).await;
        assert!(
            matches!(limited, Err(ResilienceError::Limited { .. })),
            "{limited:?}"
        );
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(rate.run(|_| answering()).await.is_ok(), "its token waited");

        let bulkhead = policy(ResilienceConfig {
            concurrency: Some(1),
            ..ResilienceConfig::default()
        });
        let held = bulkhead.begin(None).await.expect("the one slot");
        let full = bulkhead.run(|_| answering()).await;
        assert!(
            matches!(full, Err(ResilienceError::Full { .. })),
            "{full:?}"
        );
        drop(held);

        bulkhead
            .open_breaker(Duration::from_secs(30))
            .expect("a breaker");
        let open = bulkhead.run(|_| answering()).await;
        assert!(
            matches!(open, Err(ResilienceError::Open { .. })),
            "{open:?}"
        );
        let tracking =
            crate::resilient::resilient_method(|| "ShipmentsService", "track", None, None);
        bulkhead.falling_back(&tracking, &Refused);

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
        const MOVED: &str = crate::breaker::MOVED;
        type Polled = Answer<(), Refused>;
        let policy = policy(ResilienceConfig {
            retries: Some(1),
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(1),
            ..ResilienceConfig::default()
        });
        let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(1)).await;

        let lines = nest_rs_testing::LogCapture::install();
        let mut events = policy.events();
        let once = policy
            .run_once::<Polled, _, _, _>(
                None,
                judging(PhantomData::<Polled>, every, none, none),
                |_| async { Err(Refused) },
            )
            .await;
        let sent = AtomicU32::new(0);
        let again = policy
            .run_again::<Polled, _, _, _>(
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
            let line = lines.expect_one(TARGET, crate::breaker::MOVED);
            assert_eq!(line.field("state").as_deref(), Some("open"), "{line:?}");
            line.field("error")
        };

        let lines = nest_rs_testing::LogCapture::install();
        let failed = opening()
            .run(|_| async { Err::<(), _>(unreachable()) })
            .await;
        assert!(
            matches!(failed, Err(ResilienceError::Failed(_))),
            "{failed:?}"
        );
        assert_eq!(opened(&lines).as_deref(), Some(UNREACHABLE));
        drop(lines);

        let lines = nest_rs_testing::LogCapture::install();
        let again = opening()
            .run_idempotent(|_| async { Err::<(), _>(unreachable()) })
            .await;
        assert!(
            matches!(again, Err(ResilienceError::Failed(_))),
            "{again:?}"
        );
        assert_eq!(opened(&lines).as_deref(), Some(UNREACHABLE));
        drop(lines);

        let lines = nest_rs_testing::LogCapture::install();
        let timed_out = opening()
            .run(|_| std::future::pending::<Result<(), Refused>>())
            .await;
        assert!(
            matches!(timed_out, Err(ResilienceError::TimedOut { .. })),
            "{timed_out:?}"
        );
        assert_eq!(opened(&lines).as_deref(), Some(TIMED_OUT));
        drop(lines);

        let lines = nest_rs_testing::LogCapture::install();
        let policy = opening();
        let unwound = nest_rs_core::panic::contain(policy.run(|_| async {
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
            if self.policy.breaker.as_deref().is_some_and(Breaker::locked) {
                self.under_the_lock.fetch_add(1, Ordering::Relaxed);
            }
            f.write_str("the upstream refused, watching")
        }
    }

    impl Error for Watching {}

    /// A call's error is user code: it is said once per failed attempt, for
    /// the breaker's opening and the retry's line alike, and before the
    /// breaker is asked, never under its lock — where an error reading its
    /// policy's breaker would wait for good.
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
            .run_idempotent(|_| async {
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
            .run_idempotent(|_| async { Err::<(), _>(Refused) })
            .await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));

        let timed_out = policy(ResilienceConfig {
            timeout: Duration::from_millis(100),
            retries: None,
            breaker_window: None,
            ..ResilienceConfig::default()
        })
        .run(|_| std::future::pending::<Result<(), Refused>>())
        .await;
        assert!(matches!(timed_out, Err(ResilienceError::TimedOut { .. })));

        let opening = policy(ResilienceConfig {
            retries: Some(1),
            breaker_window: Some(1),
            breaker_failures: 1,
            ..ResilienceConfig::default()
        });
        let open = opening
            .run_idempotent(|_| async { Err::<(), _>(Refused) })
            .await;
        assert!(
            matches!(open, Err(ResilienceError::Open { .. })),
            "its retry, refused unsent by the breaker its failure opened: {open:?}"
        );

        let narrow = policy(ResilienceConfig {
            concurrency: Some(1),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        let held = narrow.begin(None).await.expect("the one slot");
        let full = narrow.run(|_| async { Ok::<_, Refused>(()) }).await;
        assert!(matches!(full, Err(ResilienceError::Full { .. })));
        drop(held);

        let waiting = policy(ResilienceConfig {
            concurrency: Some(1),
            concurrency_wait: Some(Duration::from_secs(60)),
            concurrency_waiters: Some(1),
            breaker_window: None,
            ..ResilienceConfig::default()
        });
        let held = waiting.begin(None).await.expect("the one slot");
        let mut queued = Box::pin(waiting.run(|_| async { Ok::<_, Refused>(()) }));
        park(&mut queued).await;
        let crowded = waiting.run(|_| async { Ok::<_, Refused>(()) }).await;
        assert!(
            matches!(crowded, Err(ResilienceError::Full { .. })),
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
        assert!(spent.run(|_| async { Ok::<_, Refused>(()) }).await.is_ok());
        let held = spent.begin(None).await.expect("the one slot");
        let read = spent.run(|_| async { Ok::<_, Refused>(()) }).await;
        assert!(
            matches!(read, Err(ResilienceError::Limited { .. })),
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
            .run_idempotent(|_| async { Err::<(), _>(Refused) })
            .await;
        assert!(
            matches!(limited, Err(ResilienceError::Limited { .. })),
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
            gated.run(|_| async { Ok::<_, Refused>(()) }),
        )
        .await;
        assert!(
            matches!(spent, Err(ResilienceError::TimedOut { .. })),
            "a unit out of time, refused before the bulkhead: {spent:?}"
        );
        let failed = gated.run(|_| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        let open = gated.run(|_| async { Ok::<_, Refused>(()) }).await;
        assert!(
            matches!(open, Err(ResilienceError::Open { .. })),
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

    #[test]
    fn a_first_wait_above_the_ceiling_is_refused_naming_both_variables() {
        let refused = ResiliencePolicy::<Upstream>::new(&ResilienceConfig {
            backoff_first: Duration::from_secs(3),
            ..ResilienceConfig::default()
        })
        .expect_err("a first wait above the longest")
        .to_string();
        assert!(
            refused.contains(&nest_rs_config::var_name(
                "resilience__upstream",
                "BACKOFF_FIRST_MS"
            )) && refused.contains(&nest_rs_config::var_name(
                "resilience__upstream",
                "BACKOFF_CEILING_MS"
            )),
            "{refused}"
        );
    }

    #[test]
    fn more_failures_than_the_window_holds_is_refused_naming_both_variables() {
        let refused = ResiliencePolicy::<Unnamed>::new(&ResilienceConfig {
            breaker_window: Some(5),
            breaker_failures: 6,
            ..ResilienceConfig::default()
        })
        .expect_err("a breaker that never opens")
        .to_string();
        assert!(
            refused.contains(&nest_rs_config::var_name("resilience", "BREAKER_FAILURES"))
                && refused.contains(&nest_rs_config::var_name("resilience", "BREAKER_WINDOW")),
            "{refused}"
        );
    }

    /// Under a rate limit, a breaker opening on more failures than the
    /// policy's attempts can count within its age of one another is opened by
    /// none of them, and is refused naming the settings that decide it,
    /// quoting none of their values, and the one outcome the check does not
    /// bound, a response body the middleware's caller holds. Three attempts
    /// an hour let three answer within a minute at most — a full bucket's, the
    /// next token twenty minutes off, past that minute and the ten an attempt
    /// takes at most — and a breaker opening on three is built, and opens on
    /// them.
    #[tokio::test(start_paused = true)]
    async fn more_failures_than_the_rate_lets_answer_within_the_age_is_refused() {
        let config = |failures| ResilienceConfig {
            retries: None,
            breaker_window: Some(20),
            breaker_failures: failures,
            breaker_age: Some(Duration::from_secs(60)),
            rate_limit: Some(3),
            rate_window: Duration::from_secs(3600),
            ..ResilienceConfig::default()
        };
        let refused = ResiliencePolicy::<Upstream>::new(&config(4))
            .expect_err("a breaker the policy's attempts never open")
            .to_string();
        assert!(
            refused.contains("the policy's attempts") && refused.contains("ResilienceMiddleware"),
            "what the check bounds, and what it does not: {refused}"
        );
        let mut unnamed = refused.clone();
        for key in [
            "BREAKER_FAILURES",
            "BREAKER_AGE_SECS",
            "RATE_LIMIT",
            "RATE_WINDOW_SECS",
        ] {
            let var = nest_rs_config::var_name("resilience__upstream", key);
            assert!(unnamed.contains(&var), "{refused}");
            unnamed = unnamed.replace(&var, "");
        }
        assert!(
            !unnamed.contains(|c: char| c.is_ascii_digit()),
            "no value is quoted: {refused}"
        );

        let policy = policy(config(3));
        for _ in 0..3 {
            let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
            assert!(
                matches!(failed, Err(ResilienceError::Failed(Refused))),
                "{failed:?}"
            );
        }
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "three failures at once, from a bucket left to fill"
        );
    }

    /// The most failures that answer within the age of one another are a full
    /// bucket's and every token back while the age and the longest attempt
    /// last. Three attempts every ten minutes, with a ten minutes' age, let
    /// nine: three cut at the ten minutes an attempt takes at most, three sent
    /// as they are cut, the bucket full again, and three once it has filled
    /// anew, the age later. A breaker opening on nine is built and opens on
    /// them; one opening on ten is refused.
    #[tokio::test(start_paused = true)]
    async fn more_failures_than_a_full_bucket_and_the_tokens_back_in_the_span_send_is_refused() {
        let window = Duration::from_secs(600);
        let config = |failures| ResilienceConfig {
            timeout: LONGEST_ATTEMPT,
            retries: None,
            breaker_window: Some(20),
            breaker_failures: failures,
            breaker_age: Some(window),
            rate_limit: Some(3),
            rate_window: window,
            ..ResilienceConfig::default()
        };
        assert!(
            ResiliencePolicy::<Upstream>::new(&config(10)).is_err(),
            "a breaker the policy's attempts never open"
        );

        let policy = Arc::new(policy(config(9)));
        let cut: Vec<_> = (0..3)
            .map(|_| {
                let policy = Arc::clone(&policy);
                tokio::spawn(async move {
                    policy
                        .run(|_| std::future::pending::<Result<(), Refused>>())
                        .await
                })
            })
            .collect();
        for cut in cut {
            let cut = cut.await.expect("the call's task");
            assert!(
                matches!(cut, Err(ResilienceError::TimedOut { .. })),
                "{cut:?}"
            );
        }
        let failed = || async {
            let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
            assert!(
                matches!(failed, Err(ResilienceError::Failed(Refused))),
                "{failed:?}"
            );
        };
        for _ in 0..3 {
            failed().await;
        }
        tokio::time::advance(window).await;
        for _ in 0..3 {
            failed().await;
        }
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "nine failures, the first and the last the age apart"
        );
    }

    /// The failures counted together may come from attempts sent long before
    /// they answer: one attempt a minute, each running up to the ten minutes
    /// an attempt takes at most, lets twelve answer within a minute of one
    /// another. A breaker opening on twelve is built, one on thirteen refused,
    /// and eleven sent a minute apart open it as they answer.
    #[tokio::test(start_paused = true)]
    async fn a_breaker_its_rate_paces_opens_on_failures_sent_far_apart_that_answer_together() {
        let config = |failures| ResilienceConfig {
            timeout: LONGEST_ATTEMPT,
            retries: None,
            breaker_window: Some(20),
            breaker_failures: failures,
            breaker_age: Some(Duration::from_secs(60)),
            rate_limit: Some(1),
            rate_window: Duration::from_secs(60),
            ..ResilienceConfig::default()
        };
        assert!(ResiliencePolicy::<Upstream>::new(&config(13)).is_err());
        assert!(ResiliencePolicy::<Upstream>::new(&config(12)).is_ok());

        let policy = Arc::new(policy(config(11)));
        let answers = tokio::time::Instant::now() + Duration::from_secs(600);
        let sent: Vec<_> = (1..=11)
            .map(|nth| {
                let policy = Arc::clone(&policy);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(60) * nth).await;
                    policy
                        .run(|_| async move {
                            tokio::time::sleep_until(answers).await;
                            Err::<(), _>(Refused)
                        })
                        .await
                })
            })
            .collect();
        for sent in sent {
            let failed = sent.await.expect("the call's task");
            assert!(
                matches!(failed, Err(ResilienceError::Failed(Refused))),
                "{failed:?}"
            );
        }
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "ten failures at 600 s and the last at 660 s, within the minute"
        );
    }

    /// Each combination the policy refuses names every setting beside the one
    /// refused both ways it is set, its variable and its field in code, so a
    /// policy pinned through `ResilienceModule::for_root` is told what it set.
    #[test]
    fn a_refused_combination_names_each_setting_beside_it_by_its_variable_and_its_field() {
        let namespace = instance_namespace::<Upstream>(ResilienceConfig::NAMESPACE);
        let refused = |config: ResilienceConfig| {
            ResiliencePolicy::<Upstream>::new(&config)
                .expect_err("a combination that cannot work")
                .to_string()
        };
        for (said, beside) in [
            (
                refused(ResilienceConfig {
                    backoff_first: Duration::from_secs(3),
                    ..ResilienceConfig::default()
                }),
                vec![ResilienceConfig::BACKOFF_CEILING.var_or_field(&namespace)],
            ),
            (
                refused(ResilienceConfig {
                    breaker_window: Some(5),
                    breaker_failures: 6,
                    ..ResilienceConfig::default()
                }),
                vec![ResilienceConfig::BREAKER_WINDOW.var_or_field(&namespace)],
            ),
            (
                refused(ResilienceConfig {
                    breaker_age: Some(Duration::from_secs(60)),
                    rate_limit: Some(3),
                    rate_window: Duration::from_secs(3600),
                    ..ResilienceConfig::default()
                }),
                vec![
                    ResilienceConfig::BREAKER_AGE.var_or_field(&namespace),
                    ResilienceConfig::RATE_LIMIT.var_or_field(&namespace),
                    ResilienceConfig::RATE_WINDOW.var_or_field(&namespace),
                ],
            ),
        ] {
            for setting in beside {
                assert!(said.contains(&setting), "{setting} in: {said}");
            }
        }
    }

    /// An age and a rate that leave a breaker no room to open mean nothing
    /// without a breaker, as a slot's wait means nothing without slots: the
    /// settings refused beside the default breaker build a policy with none.
    #[test]
    fn an_age_a_rate_leaves_no_room_for_refuses_nothing_without_a_breaker() {
        let config = ResilienceConfig {
            breaker_age: Some(Duration::from_secs(60)),
            rate_limit: Some(3),
            rate_window: Duration::from_secs(3600),
            ..ResilienceConfig::default()
        };
        assert!(
            ResiliencePolicy::<Upstream>::new(&config).is_err(),
            "ten failures under three attempts an hour, with the default breaker"
        );
        let policy = ResiliencePolicy::<Upstream>::new(&ResilienceConfig {
            breaker_window: None,
            ..config
        })
        .expect("no breaker for the age to leave no room in");
        assert!(policy.breaker.is_none() && policy.rate.is_some());
    }

    #[test]
    fn an_unmet_policy_names_its_instances_import() {
        assert_eq!(
            ResiliencePolicy::<Upstream>::remedy(),
            format!(
                "Import `nest_rs::resilience::ResilienceModule<Upstream>`, which builds the \
                 policy from {}.",
                nest_rs_config::var_name("resilience__upstream", "*")
            )
        );
        assert_eq!(
            ResiliencePolicy::<Unnamed>::remedy(),
            format!(
                "Import `nest_rs::resilience::ResilienceModule`, which builds the policy from {}.",
                nest_rs_config::var_name("resilience", "*")
            )
        );
    }

    /// A policy with no breaker says so by type, never as a breaker closed,
    /// and refuses either switch; an age and probes mean nothing without a
    /// breaker, as a slot's wait means nothing without slots, and build none.
    #[tokio::test(start_paused = true)]
    async fn a_policy_with_no_breaker_says_so_by_type_and_refuses_either_switch() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(ResilienceConfig {
            breaker_window: None,
            breaker_age: Some(Duration::from_secs(60)),
            breaker_probes: 3,
            ..ResilienceConfig::default()
        });
        assert!(policy.breaker.is_none());
        assert_eq!(policy.breaker_state(), None);
        assert_eq!(
            policy.open_breaker(Duration::from_secs(10)),
            Err(BreakerError::Off)
        );
        assert_eq!(
            policy.open_breaker(Duration::ZERO),
            Err(BreakerError::Off),
            "no breaker before a duration it would not take"
        );
        assert_eq!(policy.close_breaker(), Err(BreakerError::Off));
        assert_eq!(
            BreakerError::Off.to_string(),
            "the policy has no circuit breaker to switch"
        );
        assert!(policy.run(|_| answering()).await.is_ok());
        assert!(lines.find(TARGET, crate::breaker::MOVED).is_empty());
    }

    /// A manual opening for a duration `breaker_open` would not take in code
    /// — none, or past its ceiling — is refused, never clamped: the breaker
    /// stays as it was and files nothing, and the refusal names the open times
    /// it takes and the field taking them, never the duration given. Anything
    /// above none up to the ceiling is taken, a time under a second included.
    #[tokio::test(start_paused = true)]
    async fn a_manual_opening_outside_the_open_times_breaker_open_takes_is_refused_unclamped() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(ResilienceConfig::default());
        let bounds = ResilienceConfig::BREAKER_OPEN.bounds();
        assert!(
            matches!(bounds.least(), nest_rs_config::Floor::AboveZero(_)),
            "an open time is anything above none"
        );
        let most = bounds.unit().duration(bounds.most().count);
        let tick = Duration::from_nanos(1);
        for outside in [Duration::ZERO, most + tick, Duration::MAX] {
            let refused = policy
                .open_breaker(outside)
                .expect_err("outside the open times");
            assert_eq!(refused, BreakerError::OutOfRange { most });
            let said = refused.to_string();
            assert_eq!(
                said,
                format!(
                    "a breaker opened by hand stays open for a time above zero, up to {most:?}, \
                     the open times `{}` takes in code",
                    ResilienceConfig::BREAKER_OPEN.field().path()
                )
            );
            assert!(!said.contains(&format!("{outside:?}")), "{said}");
        }
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
        assert!(lines.find(TARGET, crate::breaker::MOVED).is_empty());
        for taken in [tick, Duration::from_millis(500), most] {
            assert_eq!(policy.open_breaker(taken), Ok(()), "{taken:?}");
            assert_eq!(
                policy.breaker_state(),
                Some(BreakerState::Open { retry_after: taken })
            );
        }
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
        let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        let into = Duration::from_millis(200);
        tokio::time::advance(into).await;
        let Err(refused) = policy.run(|_| answering()).await else {
            panic!("open for 500 ms");
        };
        let ResilienceError::Open { retry_after, .. } = &refused else {
            panic!("refused by the breaker: {refused:?}");
        };
        assert_eq!(*retry_after, open - into);
        let problem = nest_rs_core::__private::find_in_chain::<Problem>(&refused)
            .expect("the problem rides the chain");
        assert_eq!(problem.retry_after(), Some(1), "a second, rounded up");
        tokio::time::advance(open - into).await;
        assert!(policy.run(|_| answering()).await.is_ok(), "the probe");
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));

        policy
            .open_breaker(open)
            .expect("an open time `breaker_open` takes");
        tokio::time::advance(open - Duration::from_millis(1)).await;
        assert!(
            matches!(
                policy.run(|_| answering()).await,
                Err(ResilienceError::Open { .. })
            ),
            "a millisecond short of the opening's end"
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(policy.run(|_| answering()).await.is_ok(), "the probe");
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
        assert!(policy.run(|_| failing()).await.is_err());
        tokio::time::advance(age + Duration::from_millis(1)).await;
        assert!(policy.run(|_| failing()).await.is_err());
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Closed),
            "the first failure is past the age"
        );
        tokio::time::advance(age).await;
        assert!(policy.run(|_| failing()).await.is_err());
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
        assert!(policy.run(|_| sent()).await.is_ok());
        let into = Duration::from_millis(100);
        tokio::time::advance(into).await;
        assert!(policy.run(|_| sent()).await.is_ok());
        let Err(refused) = policy.run(|_| sent()).await else {
            panic!("past the limit");
        };
        let ResilienceError::Limited { retry_after, .. } = &refused else {
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
        assert!(policy.run(|_| sent()).await.is_ok(), "the token is back");
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
        let refused = policy.run_idempotent(|_| sent()).await;
        assert!(
            matches!(refused, Err(ResilienceError::Open { retry_after, .. })
                if retry_after == Duration::from_secs(15)),
            "{refused:?}"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 0, "refused unsent");
        policy.close_breaker().expect("a breaker");
        assert!(policy.run(|_| sent()).await.is_ok());
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
        let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(1)).await;
        let sent = AtomicU32::new(0);
        let mut probing = Box::pin(policy.run(|_| async {
            sent.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Refused>(())
        }));
        park(&mut probing).await;
        assert_eq!(policy.breaker_state(), Some(BreakerState::HalfOpen));
        let beside = policy.run(|_| answering()).await;
        assert!(
            matches!(beside, Err(ResilienceError::Open { retry_after, .. })
                if retry_after == Duration::from_secs(14)),
            "at 1 s, the probe sent at its token's instant, 10 s, within its 5 s timeout: {beside:?}"
        );

        policy
            .open_breaker(Duration::from_secs(30))
            .expect("an open time `breaker_open` takes");
        let refused = probing.await;
        assert!(
            matches!(refused, Err(ResilienceError::Open { retry_after, .. })
                if retry_after == Duration::from_secs(21)),
            "at 10 s, what is left of the 30 s it was opened for at 1 s: {refused:?}"
        );
        assert_eq!(sent.load(Ordering::Relaxed), 0, "the probe is never sent");
        policy.close_breaker().expect("a breaker");
        let resumed = tokio::time::Instant::now();
        assert!(policy.run(|_| answering()).await.is_ok());
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
        let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(1)).await;
        let sent = AtomicU32::new(0);
        let mut probing = Box::pin(policy.run(|_| async {
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
            .find(TARGET, crate::breaker::MOVED)
            .iter()
            .map(|line| line.field("state").unwrap_or_default())
            .collect();
        assert_eq!(
            states,
            ["open", "open", "half_open", "closed"],
            "opened by a failure, then by hand, then probed at its token's instant"
        );
    }

    /// The policy's breaker forgets an outcome past `breaker_age`, and closes on
    /// `breaker_probes` probes in a row.
    #[tokio::test(start_paused = true)]
    async fn a_policy_builds_its_breaker_with_the_age_and_the_probes_it_reads() {
        let policy = policy(ResilienceConfig {
            retries: None,
            breaker_window: Some(4),
            breaker_failures: 2,
            breaker_open: Duration::from_secs(10),
            breaker_age: Some(Duration::from_secs(60)),
            breaker_probes: 2,
            ..ResilienceConfig::default()
        });
        let failing = || async { Err::<(), _>(Refused) };
        assert!(policy.run(|_| failing()).await.is_err());
        tokio::time::advance(Duration::from_secs(61)).await;
        assert!(policy.run(|_| failing()).await.is_err());
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Closed),
            "the first failure is past the age"
        );
        assert!(policy.run(|_| failing()).await.is_err());
        assert!(matches!(
            policy.breaker_state(),
            Some(BreakerState::Open { .. })
        ));
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(policy.run(|_| answering()).await.is_ok());
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::HalfOpen),
            "one probe of two"
        );
        assert!(policy.run(|_| answering()).await.is_ok());
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
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
        assert!(policy.run(|_| answering()).await.is_ok());
        let mut ahead = Box::pin(policy.run(|_| answering()));
        park(&mut ahead).await;
        policy
            .open_breaker(Duration::from_secs(1))
            .expect("a breaker");
        tokio::time::advance(Duration::from_secs(1)).await;
        let mut probe = Box::pin(policy.run(|_| answering()));
        park(&mut probe).await;
        let told = async || match policy.run(|_| answering()).await {
            Err(ResilienceError::Open { retry_after, .. }) => retry_after.as_secs(),
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
        matches!(cut, Err(ResilienceError::TimedOut { timeout: at, .. }) if *at == timeout)
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
                    outer.run(|_| async {
                        if idempotent {
                            inner.run_idempotent(|_| silent()).await
                        } else {
                            inner.run(|_| silent()).await
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
            let nested = outer.run(|_| inner.run(|_| silent()));
            let deadline = Instant::now() + soon;
            if edge_first {
                let cut = cut_first_at(deadline, nested).await;
                assert!(cut.is_none(), "the edge cut the unit first: {cut:?}");
            } else {
                let cut = nest_rs_core::__private::with_deadline(Some(deadline), nested).await;
                assert!(
                    matches!(
                        cut,
                        Err(ResilienceError::Failed(ResilienceError::TimedOut { timeout, .. }))
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
        let cut = outer.run(|_| inner.run(|_| silent())).await;
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
        let slot = for_a_slot.begin(None).await.expect("the one slot");
        let cut = outer.run(|_| for_a_slot.run(|_| answering())).await;
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
        let failed = for_a_token.run(|_| async { Err::<(), _>(Refused) }).await;
        assert!(matches!(failed, Err(ResilienceError::Failed(Refused))));
        tokio::time::advance(Duration::from_secs(1)).await;
        let cut = outer.run(|_| for_a_token.run(|_| answering())).await;
        assert!(timed_out_at(&cut, short), "{cut:?}");
        assert_eq!(
            for_a_token.breaker_state(),
            Some(BreakerState::HalfOpen),
            "the probe's turn is free, nothing counted against it"
        );
        let probed = for_a_token.run(|_| answering()).await;
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
            .run(|_| backing_off.run_idempotent(|_| async { Err::<(), _>(Refused) }))
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
            .run(|_| async move {
                let task = tokio::spawn(async move { spawned.run(|_| silent()).await });
                task.await.expect("the task is not cancelled")
            })
            .await;
        assert!(timed_out_at(&cut, short), "{cut:?}");
        assert_eq!(counted(&inner), (0, 0), "the task's attempt goes on");
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert_eq!(counted(&inner), (1, 1), "given up at its own bound, at 5 s");

        let dropped = tokio::time::timeout(short, inner.run(|_| silent())).await;
        assert!(dropped.is_err(), "its caller dropped it");
        assert_eq!(
            counted(&inner),
            (1, 1),
            "a caller's drop counts for nothing"
        );
    }

    /// The attempts of two policies in flight together in a call the timeout
    /// of a third drops have each timed out, as have the ones of two nested
    /// one in the other's — and one of the same policy nested in its own
    /// attempt, counted before the attempt around it: two timeouts, which
    /// open the breaker.
    #[tokio::test(start_paused = true)]
    async fn every_attempt_in_flight_in_a_call_given_up_at_its_bound_has_timed_out() {
        let short = Duration::from_secs(2);
        let (first, second) = (policy(guarding()), policy(guarding()));
        let cut = around(short)
            .run(|_| async {
                let (first, second) =
                    tokio::join!(first.run(|_| silent()), second.run(|_| silent()));
                first.and(second)
            })
            .await;
        assert!(timed_out_at(&cut, short), "{cut:?}");
        assert_eq!((counted(&first), counted(&second)), ((1, 1), (1, 1)));

        let (middle, innermost) = (policy(guarding()), policy(guarding()));
        let cut = around(short)
            .run(|_| middle.run(|_| innermost.run(|_| silent())))
            .await;
        assert!(timed_out_at(&cut, short), "{cut:?}");
        assert_eq!((counted(&middle), counted(&innermost)), ((1, 1), (1, 1)));

        let lines = nest_rs_testing::LogCapture::install();
        let nested = policy(guarding());
        let cut = nested
            .run(|_| async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                nested.run(|_| silent()).await
            })
            .await;
        assert!(timed_out_at(&cut, Duration::from_secs(5)), "{cut:?}");
        let opened = lines.find(TARGET, BREAKER_MOVED);
        assert_eq!(opened.len(), 1, "{opened:?}");
        assert_eq!(
            (opened[0].field("failed"), opened[0].field("error")),
            (Some("2".to_owned()), Some(TIMED_OUT.to_owned())),
            "the inner attempt, due a second later, then the one around it"
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
            .run(|_| async {
                let run = inner.begin(None).await.expect("the request is let through");
                let (attempted, _outstanding) = run.attempt_once(|_| silent(), judged).await;
                attempted
                    .map(drop)
                    .map_err(ResilienceError::<Refused>::from)
            })
            .await;
        assert!(timed_out_at(&cut, short), "{cut:?}");
        assert_eq!(counted(&inner), (1, 1));
    }

    /// A unit of work whose transaction settles the savepoints it opens after
    /// a wait, as a round trip to the database takes one, or never can.
    struct Settling(Result<Duration, ()>);

    impl nest_rs_database::Executor for Settling {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn savepoint(&self) -> Option<Arc<dyn nest_rs_database::AtomicBlock>> {
            Some(Arc::new(Self(self.0)))
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

    /// The run of a call that never answers, in a unit holding `transaction`:
    /// declared idempotent, or sent once by a method declaring a fallback,
    /// each of which opens a savepoint.
    fn silent_in<'p>(
        policy: &'p ResiliencePolicy<Upstream>,
        transaction: Settling,
        idempotent: bool,
    ) -> Pin<Box<dyn Future<Output = Ran> + 'p>> {
        let falling_back =
            crate::resilient::resilient_method(|| "QuotesService", "quote", None, None)
                .with_fallback();
        let run: Pin<Box<dyn Future<Output = Ran> + 'p>> = if idempotent {
            Box::pin(policy.run_idempotent(|_| silent()))
        } else {
            Box::pin(policy.run_once::<Ran, _, _, _>(
                Some(falling_back),
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
    /// though its attempt timed out: that timeout is the attempt's, counted
    /// and told as it ended, and the run's answer says what its unit holds.
    #[tokio::test(start_paused = true)]
    async fn a_timed_out_attempt_whose_savepoint_cannot_settle_is_told_then_ends_unsettled() {
        for idempotent in [false, true] {
            let policy = policy(guarding());
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
                "idempotent: {idempotent}: {heard:?}"
            );
        }
    }
}
