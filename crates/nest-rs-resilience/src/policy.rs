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
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use nest_rs_config::{ConfigError, Namespaced, instance_namespace};
use nest_rs_core::__private::WithinHeld;
use nest_rs_core::{Backoff, Instance, Problem, Unanswered, Unnamed, process_seed};
use tokio::sync::SemaphorePermit;
use tokio::time::{Instant, Sleep};
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::TARGET;
use crate::breaker::{Breaker, BreakerState, Pass, Sent};
use crate::bulkhead::Bulkhead;
use crate::call::{Again, Call, Once};
use crate::config::{LONGEST_ATTEMPT, ResilienceConfig, is_open_time};
use crate::counters::Counters;
use crate::error::{BreakerError, Refusal, ResilienceError};
use crate::millis::millis;
use crate::rate::Rate;
use crate::resilient::{ResilientAnswer, ResilientMethod};

/// The calls to one dependency — a payment provider, a carrier's API, a
/// language model — run under one policy, by every caller in the process.
///
/// Injected as `Arc<ResiliencePolicy<I>>` once a module imports
/// [`ResilienceModule<I>`](crate::ResilienceModule), which builds it from
/// `<PREFIX>_RESILIENCE__<INSTANCE>__*` ([`ResilienceConfig`]); the default
/// instance, `ResiliencePolicy`, reads `<PREFIX>_RESILIENCE__*`. A method runs
/// its body under it through [`#[resilient]`](macro@crate::resilient), naming
/// the field that holds it; [`run`](Self::run) and
/// [`run_idempotent`](Self::run_idempotent) take a call that needs its
/// attempt's [`CancellationToken`], or is part of a method.
///
/// Each attempt is given up past the policy's timeout or the deadline of the
/// unit it runs in (`nest_rs_core::Deadline`), whichever comes first, and its
/// [`CancellationToken`] cancelled, so work the call handed to another task
/// stops with it; no retry starts whose wait ends past that deadline. A call is attempted again only when it is declared
/// idempotent, through [`run_idempotent`](Self::run_idempotent): replaying one
/// that is not would charge a card twice. Every attempt's outcome counts in the
/// circuit breaker, which refuses every call at once while it is open, the
/// bulkhead bounds how many calls run inside the policy together, and the
/// rate limit how many attempts it sends per window, a retry included.
///
/// Every `Err` the call answers counts as the dependency failing. An answer
/// the dependency gave — a declined card, a parcel not found — belongs in the
/// call's `Ok`, so it is neither retried nor counted.
///
/// The breaker's state is read through [`breaker_state`](Self::breaker_state),
/// and switched by hand — an operator taking a dependency out for its
/// maintenance window — through [`open_breaker`](Self::open_breaker) and
/// [`close_breaker`](Self::close_breaker).
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
    breaker: Option<Breaker>,
    bulkhead: Option<Bulkhead>,
    rate: Option<Rate>,
    counters: Arc<Counters>,
    /// Sets each run's jitter apart from the others'.
    runs: AtomicU64,
    _instance: PhantomData<fn() -> I>,
}

/// How an attempt's answer counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Succeeded,
    /// The dependency failed, asking the wait before a retry when it said one.
    Failed {
        asked: Option<Duration>,
    },
}

/// One attempt's answer and how it counts, or the policy's refusal.
pub(crate) type Attempted<T> = Result<(T, Verdict), Refusal>;

/// One run of a call under its policy, holding its bulkhead slot.
pub(crate) struct Run<'p, I> {
    /// What each of its attempts is sent under.
    terms: Terms<'p, I>,
    _slot: Option<SemaphorePermit<'p>>,
    /// How many times the run may send its call again: the policy's retries,
    /// or the ones its method pinned.
    retries: u32,
    /// Drawn at the first retry, so a call answering at once draws none.
    seed: Option<u64>,
    /// How many times this run attempted its call again so far, a retry the
    /// policy refused unsent included.
    pub(crate) resent: u32,
}

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
    /// that cannot work, named as the instance's variables.
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
            .map(|limit| Rate::new(limit, config.rate_window, config.rate_wait));
        // An outcome's age runs from its answer, which comes up to the longest
        // attempt after the rate counted its call.
        if let (Some(_), Some(age), Some(rate)) = (config.breaker_window, config.breaker_age, &rate)
            && config.breaker_failures > rate.most_sent_within(age + LONGEST_ATTEMPT)
        {
            return Err(ConfigError::parse(
                ResilienceConfig::BREAKER_FAILURES.var(&namespace),
                format!(
                    "must be at most the failures that can answer within the age of one another \
                     under the rate limit, set by {}; {}; and {}: a breaker opening on more never \
                     opens",
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
        Ok(Self {
            timeout: config.timeout,
            retries,
            backoff: Backoff::new(config.backoff_first, config.backoff_ceiling),
            breaker: config.breaker_window.map(|window| {
                Breaker::new(
                    I::NAME,
                    window,
                    config.breaker_failures,
                    config.breaker_open,
                    config.breaker_age,
                    config.breaker_probes,
                    Arc::clone(&counters),
                )
            }),
            bulkhead: config.concurrency.map(|slots| {
                Bulkhead::new(slots, config.concurrency_wait, config.concurrency_waiters)
            }),
            rate,
            counters,
            runs: AtomicU64::new(0),
            _instance: PhantomData,
        })
    }

    /// Run `call` once, under the policy's timeout, breaker, bulkhead and rate
    /// limit, and never again: a call this runs may change what it calls, so a
    /// retry could do it twice.
    ///
    /// `call` is handed the attempt's [`CancellationToken`], cancelled when the
    /// attempt is given up.
    pub fn run<T, E, F, Fut>(&self, call: F) -> impl Future<Output = Result<T, ResilienceError<E>>>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        self.run_once::<Result<T, ResilienceError<E>>, F, Fut>(None, call)
    }

    /// Run the idempotent `call`, attempting it again after a failed or timed
    /// out attempt, up to the policy's retries, each retry waiting the
    /// policy's backoff first — or longer, the `Retry-After` of a
    /// [`Problem`] its error's chain carries. An attempt the circuit breaker
    /// refuses ends the run at once, as does one past the rate limit that
    /// cannot wait for the window's reset.
    ///
    /// Idempotent means one call or several leave the dependency as one does:
    /// a read, a `PUT`, a request carrying an idempotency key the dependency
    /// honours. `call` is handed each attempt's [`CancellationToken`],
    /// cancelled when that attempt is given up.
    pub fn run_idempotent<T, E, F, Fut>(
        &self,
        call: F,
    ) -> impl Future<Output = Result<T, ResilienceError<E>>>
    where
        F: FnMut(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: Error + 'static,
    {
        self.run_again::<Result<T, ResilienceError<E>>, F, Fut>(None, call)
    }

    /// `call` run once, for `method` when a decorated method runs it, its
    /// answer read as `R` reads a body's and answered as `R`: what
    /// [`run`](Self::run) runs, with no method.
    pub(crate) fn run_once<R, F, Fut>(
        &self,
        method: Option<ResilientMethod>,
        call: F,
    ) -> impl Future<Output = R>
    where
        R: ResilientAnswer,
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = R::Attempt>,
    {
        RunOnce {
            stage: RunStage::Opening,
            call: Once::new(call),
            judge: judged::<R::Ok, R::Err>,
            attempt: Attempt::new(),
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
    /// `R`: what [`run_idempotent`](Self::run_idempotent) runs, with no method.
    pub(crate) fn run_again<R, F, Fut>(
        &self,
        method: Option<ResilientMethod>,
        call: F,
    ) -> impl Future<Output = R>
    where
        R: ResilientAnswer,
        R::Err: Error + 'static,
        F: FnMut(CancellationToken) -> Fut,
        Fut: Future<Output = R::Attempt>,
    {
        RunAgain {
            stage: RunStage::Opening,
            call: Again::new(call),
            judge: judged_asking::<R::Ok, R::Err>,
            attempt: Attempt::new(),
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
        let deadline = nest_rs_core::__private::deadline();
        let slot = match &self.bulkhead {
            Some(bulkhead) => {
                if deadline.is_some_and(|deadline| deadline <= Instant::now()) {
                    return Opened::Refused(self.refused(Refusal::TimedOut(Duration::ZERO)));
                }
                if let Some(retry_after) = self.breaker.as_ref().and_then(Breaker::refusing) {
                    return Opened::Refused(self.refused(Refusal::Open(retry_after)));
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
    /// `bulkhead` for, opened once one is freed within the unit's `deadline`:
    /// refused at once by a spent rate whose reset it could not wait for, read
    /// first and counting nothing, rather than after waiting for a slot, and
    /// refused `Full` once no slot is freed for it in time or as many calls
    /// wait for one as may.
    async fn enter<'p>(
        &'p self,
        bulkhead: &'p Bulkhead,
        method: Option<ResilientMethod>,
        deadline: Option<Instant>,
    ) -> Result<Run<'p, I>, Refusal> {
        if let Some(rate) = &self.rate {
            let now = Instant::now();
            if let Some(reset) = rate.refusing(now)
                && !waits_for_reset(now + reset, rate.waits_until(now), deadline)
            {
                refused_by_the_rate(I::NAME, method.as_ref(), reset);
                return Err(self.refused(Refusal::Limited(reset)));
            }
        }
        let slot = bulkhead.enter(deadline).await.map_err(|full| {
            tracing::debug!(
                target: TARGET,
                instance = I::NAME,
                host = method.as_ref().map(ResilientMethod::host).as_deref(),
                method = method.as_ref().map(ResilientMethod::name),
                waiters = full.waiters(),
                "bulkhead full; the call is refused",
            );
            self.refused(Refusal::Full)
        })?;
        Ok(self.opened(method, deadline, Some(slot)))
    }

    /// The run of `method` opened under the unit's `deadline`, holding `slot`.
    fn opened<'p>(
        &'p self,
        method: Option<ResilientMethod>,
        deadline: Option<Instant>,
        slot: Option<SemaphorePermit<'p>>,
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
        self.breaker.as_ref().map(Breaker::read)
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
    /// answering a success closes nothing — and a probe waiting for the rate's
    /// reset loses its turn. When its wait ends it meets the breaker as a call
    /// arriving then does: refused while the opening lasts, and taking the
    /// next probe's turn once it has passed. Each call files the transition's
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
    /// nothing once it answers — a probe answering a failure opens nothing,
    /// and no answer counts in the window forgotten. Each call files the
    /// transition's `warn`, marked `manual`, and counts on the transitions'
    /// counter marked manual, whether or not the breaker was closed already.
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

    /// `refusal`, counted.
    fn refused(&self, refusal: Refusal) -> Refusal {
        self.counters.refusal(refusal);
        refusal
    }
}

impl<'p, I: Instance> Run<'p, I> {
    /// Whether the run may still send its call again.
    #[cfg(feature = "reqwest")]
    pub(crate) const fn may_resend(&self) -> bool {
        self.resent < self.retries
    }

    /// One attempt of `call`, holding the call itself, its answer counted as
    /// `judge` reads it.
    #[cfg(feature = "reqwest")]
    pub(crate) fn attempt_once<T, E, F, Fut, J>(
        &self,
        call: F,
        judge: J,
    ) -> impl Future<Output = Attempted<Result<T, E>>> + use<'p, '_, I, T, E, F, Fut, J>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
        J: Fn(&Result<T, E>) -> Verdict,
    {
        AttemptOnce::<'p, '_, I, Result<T, ResilienceError<E>>, F, Fut, J> {
            call: Once::new(call),
            judge,
            attempt: Attempt::new(),
            wait: Wait::within(),
            run: self,
            answer: PhantomData,
        }
    }

    /// The run's last attempt, its refusal counted when it ended on one.
    pub(crate) fn finish<T>(&self, attempted: Attempted<T>) -> Attempted<T> {
        attempted.map_err(|refusal| self.terms.policy.refused(refusal))
    }

    /// Whether to attempt the call again after `attempted`, having waited the
    /// backoff when so — see [`next_wait`](Self::next_wait).
    #[cfg(any(feature = "reqwest", test))]
    pub(crate) fn again<'r, T>(
        &'r mut self,
        attempted: &Attempted<T>,
    ) -> impl Future<Output = bool> + use<'r, 'p, I, T> {
        let wait = self.next_wait(attempted);
        async move {
            let Some(wait) = wait else {
                return false;
            };
            tokio::time::sleep(wait).await;
            self.resent = self.resent.saturating_add(1);
            true
        }
    }

    /// How long to wait before attempting the call again after `attempted`, its
    /// line filed, or `None` when the run ends there: after a failure or a
    /// timeout, within the run's retries, unless the dependency asked a wait
    /// past the backoff's ceiling.
    ///
    /// It reads nothing of the answer past it, so no wait holds the answer,
    /// and a run is `Send` for an answer that is `Send` and not `Sync`, a
    /// stream's.
    fn next_wait<T>(&mut self, attempted: &Attempted<T>) -> Option<Duration> {
        let wait = match attempted {
            Ok((_, Verdict::Failed { asked })) => self.retry_wait(*asked),
            Err(Refusal::TimedOut(_)) => self.retry_wait(None),
            Ok((_, Verdict::Succeeded))
            | Err(Refusal::Open(_) | Refusal::Full | Refusal::Limited(_)) => None,
        };
        if let Some(wait) = wait {
            tracing::debug!(
                target: TARGET,
                instance = I::NAME,
                host = self.terms.host().as_deref(),
                method = self.terms.method_name(),
                attempt = self.resent.saturating_add(1),
                wait_ms = millis(wait),
                "attempt failed; retrying the call",
            );
        }
        wait
    }

    /// How long to wait before the next attempt, after one that failed asking
    /// `asked`, or `None` when the run ends there.
    fn retry_wait(&mut self, asked: Option<Duration>) -> Option<Duration> {
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
                "the dependency asked a wait past the backoff's ceiling; not attempted again",
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
                "no time left before the unit's deadline; not attempted again",
            );
            return None;
        }
        Some(wait)
    }
}

/// How long an attempt sent now may run, and, under a unit's deadline, the
/// instant that ends at: its timer is armed at that instant, so it is due with
/// the edge's cut at the deadline however long the call takes to first wait,
/// and an attempt the edge cuts there first has timed out all the same
/// ([`Sent`]).
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
    /// Past the rate's limit, postponed to the window's reset at the instant.
    Postponed(Instant),
}

impl<'p, I: Instance> Terms<'p, I> {
    /// An attempt let through, in order, by what is left of the unit's
    /// deadline, the breaker and the rate, with the bound its call is sent
    /// under; with nothing left of the deadline it is not sent, nor counted.
    /// Past the rate's limit it waits for the window's reset when what is left
    /// of the rate's wait, ending at `waits_until` from its first refusal,
    /// reaches it before the unit's deadline — a wait that is no part of the
    /// attempt — a probe's `pass` keeping its turn and telling the breaker
    /// when it has answered at the latest; otherwise it is refused, counted for
    /// nothing, told the time to that reset. With no rate, nothing of the
    /// rate's runs: no lock, no clock read, no timer.
    // Inlined, the rate's step left out of line: called, it costs a run 2 to 4 ns.
    #[inline(always)]
    fn admit(
        &self,
        pass: &mut Option<Pass<'p>>,
        waits_until: &mut Option<Instant>,
    ) -> Admission<'p> {
        let bound = self.bound();
        if bound.wait.is_zero() {
            return Admission::Refused(Refusal::TimedOut(bound.wait));
        }
        // While this one waited for the reset, another call may have opened the
        // breaker or taken the probe's turn, and a manual switch voids a turn held.
        if !pass.as_ref().is_some_and(Pass::probes) {
            match self
                .policy
                .breaker
                .as_ref()
                .map(|breaker| breaker.admit(bound.wait))
                .transpose()
            {
                Ok(admitted) => *pass = admitted,
                Err(retry_after) => return Admission::Refused(Refusal::Open(retry_after)),
            }
        }
        match &self.policy.rate {
            Some(rate) => self.count(rate, pass, waits_until, bound),
            None => Admission::Through(pass.take(), bound),
        }
    }

    /// [`admit`](Self::admit)'s step through `rate`, for an attempt the
    /// breaker let through to be sent under `bound`.
    #[inline(never)]
    fn count(
        &self,
        rate: &Rate,
        pass: &mut Option<Pass<'p>>,
        waits_until: &mut Option<Instant>,
        bound: Bound,
    ) -> Admission<'p> {
        let now = Instant::now();
        let Err(reset) = rate.count(now) else {
            return Admission::Through(pass.take(), bound);
        };
        let until = *waits_until.get_or_insert_with(|| rate.waits_until(now));
        let at = now + reset;
        if !waits_for_reset(at, until, self.deadline) {
            refused_by_the_rate(I::NAME, self.method.as_ref(), reset);
            return Admission::Refused(Refusal::Limited(reset));
        }
        if let Some(pass) = pass.as_ref() {
            pass.postpone(at, self.bound_at(at));
        }
        tracing::debug!(
            target: TARGET,
            instance = I::NAME,
            host = self.host().as_deref(),
            method = self.method_name(),
            wait_ms = millis(reset),
            "rate limit reached; waiting for the window to reset",
        );
        Admission::Postponed(at)
    }
}

// The run's futures are polled in place, each holding the call once, where an `async` fn
// or block awaiting a future it was handed keeps it twice (rust-lang/rust#62958).
pin_project_lite::pin_project! {
    /// An attempt the rate refused, waiting for the window's reset, then let
    /// through again as [`Terms::admit`] says.
    struct Admitted<'p, I> {
        terms: Terms<'p, I>,
        pass: Option<Pass<'p>>,
        waits_until: Option<Instant>,
        #[pin]
        reset: Sleep,
    }
}

impl<'p, I: Instance> Future for Admitted<'p, I> {
    type Output = Result<(Option<Pass<'p>>, Bound), Refusal>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        loop {
            ready!(this.reset.as_mut().poll(cx));
            match this.terms.admit(this.pass, this.waits_until) {
                Admission::Through(pass, bound) => return Poll::Ready(Ok((pass, bound))),
                Admission::Refused(refusal) => return Poll::Ready(Err(refusal)),
                Admission::Postponed(at) => this.reset.as_mut().reset(at),
            }
        }
    }
}

pin_project_lite::pin_project! {
    /// What a run waits on besides its call, one at a time: the bound of its
    /// attempt, whose timer is armed once the call waits, and only when they
    /// come, the slot of the bulkhead it found none free of, the rate's reset
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
/// rate's reset only when it must, then its call made and sent under its
/// bound, counted in the breaker once it answers and its token cancelled when
/// it is given up. Each step writes its fields where they are.
pub(crate) struct Attempt<'p> {
    stage: AttemptStage,
    // `sent` before `given_up`: an attempt dropped mid-call settles the
    // breaker, then cancels its token.
    sent: Option<Sent<'p>>,
    given_up: Option<DropGuard>,
    /// The token the call is handed when it is made, at its first poll.
    cancel: Option<CancellationToken>,
    bound: Duration,
}

/// Where an attempt is.
#[derive(Clone, Copy)]
enum AttemptStage {
    /// Not let through yet.
    Arriving,
    /// Waiting for the rate's window to reset.
    Admitting,
    /// Its call sent.
    Sending,
    Answered,
}

impl<'p> Attempt<'p> {
    const fn new() -> Self {
        Self {
            stage: AttemptStage::Arriving,
            sent: None,
            given_up: None,
            cancel: None,
            bound: Duration::ZERO,
        }
    }

    /// The attempt of `run` polled with `call`, its answer read as `R` reads a
    /// body's and counted as `judge` reads it. A panic in the call counts as a
    /// failure and unwinds on, for the unit to contain as it would without the
    /// policy.
    fn poll_call<I, B, C, R, J>(
        &mut self,
        run: &Run<'p, I>,
        mut wait: Pin<&mut Wait<'p, I, B>>,
        mut call: Pin<&mut C>,
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
        loop {
            let admitted = match self.stage {
                AttemptStage::Arriving => {
                    let (mut pass, mut waits_until) = (None, None);
                    match run.terms.admit(&mut pass, &mut waits_until) {
                        Admission::Through(pass, bound) => Ok((pass, bound)),
                        Admission::Refused(refusal) => Err(refusal),
                        Admission::Postponed(at) => {
                            wait.set(Wait::Admitting {
                                admitted: Admitted {
                                    terms: run.terms,
                                    pass,
                                    waits_until,
                                    reset: tokio::time::sleep_until(at),
                                },
                            });
                            self.stage = AttemptStage::Admitting;
                            continue;
                        }
                    }
                }
                AttemptStage::Admitting => {
                    let WaitProj::Admitting { admitted } = wait.as_mut().project() else {
                        unreachable!("an attempt waits for the reset it was told");
                    };
                    let admitted = ready!(admitted.poll(cx));
                    wait.set(Wait::within());
                    admitted
                }
                AttemptStage::Sending => {
                    let making = Making {
                        call: call.as_mut(),
                        cancel: &mut self.cancel,
                    };
                    let answered = ready!(wait.as_mut().bound().poll(pin!(making), cx));
                    return Poll::Ready(self.settle::<C, R, J>(call, answered, judge));
                }
                AttemptStage::Answered => unreachable!("an attempt is polled after it answered"),
            };
            match admitted {
                Ok((pass, bound)) => self.send(run, wait.as_mut(), pass, bound),
                Err(refusal) => {
                    self.stage = AttemptStage::Answered;
                    return Poll::Ready(Err(refusal));
                }
            }
        }
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
        self.stage = AttemptStage::Sending;
    }

    /// What the attempt answers, `answered` read as `R` reads a body's, judged
    /// and counted in the breaker, the future the call made dropped first.
    fn settle<C, R, J>(
        &mut self,
        call: Pin<&mut C>,
        answered: Result<R::Attempt, Unanswered>,
        judge: &J,
    ) -> Attempted<Result<R::Ok, R::Err>>
    where
        C: Call,
        R: ResilientAnswer,
        J: Fn(&Result<R::Ok, R::Err>) -> Verdict,
    {
        self.stage = AttemptStage::Answered;
        self.cancel = None;
        call.spend();
        let answered = answered.map(|attempt| {
            let answer = R::split(attempt);
            let verdict = judge(&answer);
            (answer, verdict)
        });
        if let Some(sent) = self.sent.take() {
            sent.settle(!matches!(answered, Ok((_, Verdict::Succeeded))));
        }
        let given_up = self.given_up.take();
        match answered {
            Ok(judged) => {
                let _token = given_up.map(DropGuard::disarm);
                Ok(judged)
            }
            Err(Unanswered::Panicked(payload)) => std::panic::resume_unwind(payload),
            Err(_elapsed) => Err(Refusal::TimedOut(self.bound)),
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
        attempt: Attempt<'p>,
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
    type Output = Attempted<Result<R::Ok, R::Err>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        this.attempt
            .poll_call::<_, _, _, R, J>(this.run, this.wait, this.call, this.judge, cx)
    }
}

pin_project_lite::pin_project! {
    /// What [`ResiliencePolicy::run_once`] answers: a slot of the bulkhead
    /// taken, then the run's one attempt.
    struct RunOnce<'p, I, E, B, R, F, Fut, J> {
        stage: RunStage,
        // Dropped in order: the call's future, then the attempt and the wait, each
        // handing on the turn of a probe it holds, sent or waiting for the rate's
        // reset, then the run, freeing its slot.
        #[pin]
        call: Once<F, Fut>,
        judge: J,
        attempt: Attempt<'p>,
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
                    *this.run = Some(run);
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
        let attempted = ready!(this.attempt.poll_call::<_, _, _, R, J>(
            run,
            this.wait.as_mut(),
            this.call.as_mut(),
            this.judge,
            cx
        ));
        let ran = settled(run.finish(attempted));
        *this.run = None;
        *this.stage = RunStage::Answered;
        released(this.call, this.wait);
        Poll::Ready(R::answered(ran))
    }
}

pin_project_lite::pin_project! {
    /// What [`ResiliencePolicy::run_again`] answers: a slot of the bulkhead
    /// taken, then each attempt of the run, each retry after its backoff.
    struct RunAgain<'p, I, E, B, R, F, Fut, J> {
        stage: RunStage,
        // In `RunOnce`'s order.
        #[pin]
        call: Again<F, Fut>,
        judge: J,
        attempt: Attempt<'p>,
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
                    *this.run = Some(run);
                    *this.stage = RunStage::Attempting;
                }
                Err(refusal) => {
                    *this.stage = RunStage::Answered;
                    released(this.call, this.wait);
                    return Poll::Ready(R::answered(Err(refusal.into())));
                }
            }
        }
        loop {
            let (RunStage::Attempting, Some(run)) = (*this.stage, this.run.as_mut()) else {
                unreachable!("a run is polled after it answered");
            };
            if let WaitProj::BackingOff { backoff } = this.wait.as_mut().project() {
                ready!(backoff.poll(cx));
                this.wait.set(Wait::within());
                run.resent = run.resent.saturating_add(1);
                *this.attempt = Attempt::new();
            }
            let attempted = ready!(this.attempt.poll_call::<_, _, _, R, J>(
                run,
                this.wait.as_mut(),
                this.call.as_mut(),
                this.judge,
                cx
            ));
            let Some(wait) = run.next_wait(&attempted) else {
                let ran = settled(run.finish(attempted));
                *this.run = None;
                *this.stage = RunStage::Answered;
                released(this.call, this.wait);
                return Poll::Ready(R::answered(ran));
            };
            // Dropped before the backoff, so no wait holds the answer.
            drop(attempted);
            this.wait.set(Wait::BackingOff {
                backoff: tokio::time::sleep(wait),
            });
        }
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

/// Whether an attempt waits for the window's reset at `at`: within its wait,
/// which ends at `until`, and before the unit's `deadline`, at which a call
/// sent has no time to answer.
fn waits_for_reset(at: Instant, until: Instant, deadline: Option<Instant>) -> bool {
    at <= until && deadline.is_none_or(|deadline| at < deadline)
}

/// The line a call the rate refuses files, naming `method` when a decorated
/// method ran it.
fn refused_by_the_rate(instance: &'static str, method: Option<&ResilientMethod>, reset: Duration) {
    tracing::debug!(
        target: TARGET,
        instance,
        host = method.map(ResilientMethod::host).as_deref(),
        method = method.map(ResilientMethod::name),
        retry_after_ms = millis(reset),
        "rate limit reached; the call is refused",
    );
}

fn judged<T, E>(answer: &Result<T, E>) -> Verdict {
    match answer {
        Ok(_) => Verdict::Succeeded,
        Err(_) => Verdict::Failed { asked: None },
    }
}

/// [`judged`], asking the wait a [`Problem`] in the error's chain asks.
fn judged_asking<T, E: Error + 'static>(answer: &Result<T, E>) -> Verdict {
    match answer {
        Ok(_) => Verdict::Succeeded,
        Err(error) => Verdict::Failed {
            asked: nest_rs_core::__private::find_in_chain::<Problem>(error)
                .and_then(Problem::retry_after)
                .map(|seconds| Duration::from_secs(seconds.into())),
        },
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

    use nest_rs_config::{Config, ConfigService};
    use nest_rs_core::{CarriesRemedy, instance};

    use super::*;

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
            .settle(true);

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
            policy.run_once::<Ran, _, _>(Some(tracking), |_| answering()),
            async {
                tokio::task::yield_now().await;
                let crowded = policy
                    .run_once::<Ran, _, _>(Some(tracking), |_| answering())
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

        let full = lines.find(TARGET, "bulkhead full; the call is refused");
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
        let failed: Attempted<()> = Ok(((), Verdict::Failed { asked: Some(asked) }));
        assert!(run.again(&failed).await);
        let waited = started.elapsed();
        assert!(
            waited >= asked && waited <= asked.mul_f64(1.2),
            "the ask, stretched by a fifth at most: {waited:?}"
        );
        let too_long: Attempted<()> = Ok((
            (),
            Verdict::Failed {
                asked: Some(Duration::from_secs(3)),
            },
        ));
        assert!(!run.again(&too_long).await, "past the 2 s ceiling");
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
        let failed: Attempted<()> = Ok(((), Verdict::Failed { asked: None }));

        let mut retrying = run().await;
        let started = tokio::time::Instant::now();
        assert!(retrying.again(&failed).await, "attempted again");
        assert!(
            started.elapsed() >= drawn,
            "{:?}, drawn {drawn:?}",
            started.elapsed()
        );

        let ended = nest_rs_core::__private::with_deadline(
            Some(tokio::time::Instant::now() + drawn / 2),
            async {
                let mut run = run().await;
                run.again(&failed).await
            },
        )
        .await;
        assert!(!ended, "the unit's deadline comes before the wait ends");

        let asked: Attempted<()> = Ok((
            (),
            Verdict::Failed {
                asked: Some(Duration::from_secs(3)),
            },
        ));
        let mut asking = run().await;
        assert!(!asking.again(&asked).await, "past the ceiling");

        let retried = lines.expect_one(TARGET, "attempt failed; retrying the call");
        let out_of_time = lines.expect_one(
            TARGET,
            "no time left before the unit's deadline; not attempted again",
        );
        for line in [&retried, &out_of_time] {
            assert_eq!(line.field("wait_ms").as_deref(), Some("1"), "{line:?}");
        }
        let past = lines.expect_one(
            TARGET,
            "the dependency asked a wait past the backoff's ceiling; not attempted again",
        );
        assert_eq!(
            (past.field("asked_ms"), past.field("ceiling_ms")),
            (Some("3000".to_owned()), Some("2".to_owned())),
            "{past:?}"
        );
    }

    /// No retry and no breaker, `limit` attempts per `window`, each attempt past
    /// it waiting up to `wait` for the reset, or refused at once with none: the
    /// subject is the rate.
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
        F: Future,
        F::Output: fmt::Debug,
    {
        tokio::select! {
            biased;
            answered = run.as_mut() => panic!("it parks in its wait: {answered:?}"),
            () = std::future::ready(()) => {}
        }
    }

    #[tokio::test(start_paused = true)]
    async fn every_attempt_sent_counts_against_the_rate_a_retry_included() {
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
            "the retry took the window's last attempt: {refused:?}"
        );
        assert_eq!(
            calls.load(Ordering::Relaxed),
            3,
            "a refused call is never sent"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn past_the_limit_a_call_is_refused_unsent_told_the_time_to_the_windows_reset() {
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
            Duration::from_secs(50),
            "the window the first call opened resets at 60 s"
        );
        assert_eq!(started.elapsed(), Duration::ZERO, "refused at once");
        let problem = nest_rs_core::__private::find_in_chain::<Problem>(&refused)
            .expect("the problem rides the chain");
        assert_eq!(
            (problem.status(), problem.code(), problem.retry_after()),
            (503, nest_rs_core::code::UNAVAILABLE, Some(50))
        );
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    /// Nothing left of the unit's deadline refuses before the breaker and the
    /// rate are asked: the attempt counts for nothing, and is told as a unit
    /// out of time while the rate is reached too.
    #[tokio::test(start_paused = true)]
    async fn nothing_left_of_the_units_deadline_refuses_before_the_rate_counts() {
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
            "the attempt out of time took nothing of the window"
        );
        let refused = spent().await;
        assert!(
            matches!(refused, Err(ResilienceError::TimedOut { timeout, .. }) if timeout.is_zero()),
            "no time left is said before the rate's limit: {refused:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_an_open_breaker_refuses_counts_nothing_against_the_rate() {
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
            "the probe is the window's second attempt: {probed:?}"
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
            ..rated(2, Duration::from_secs(60), None)
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
            "the window reset, and a probe is sent at last: {failed:?}"
        );
        let reopened = policy.run(|_| answering()).await;
        assert!(
            matches!(reopened, Err(ResilienceError::Open { .. })),
            "the failed probe opened the breaker again, which no refused probe closed: {reopened:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_attempt_past_the_limit_waits_for_the_reset_its_wait_reaches_then_is_sent() {
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
            "until the window's reset, at 10 s"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_reset_past_the_wait_or_at_the_units_deadline_is_not_waited() {
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
            "a reset 6 s away, past the 5 s wait: {refused:?}"
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
            "a reset at the unit's deadline leaves its call no time: {refused:?}"
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

    /// Two attempts wait for one reset; the one the next window refuses again
    /// has 5 s of its 15 s left, which do not reach the reset after.
    #[tokio::test(start_paused = true)]
    async fn the_waits_of_one_attempt_never_add_up_past_its_wait() {
        let policy = policy(rated(
            1,
            Duration::from_secs(10),
            Some(Duration::from_secs(15)),
        ));
        assert!(policy.run(|_| answering()).await.is_ok());
        let started = tokio::time::Instant::now();
        let (first, second) = tokio::join!(
            policy.run(|_| async { Ok::<_, Refused>("first") }),
            policy.run(|_| async { Ok::<_, Refused>("second") }),
        );
        let mut answered = [first, second].map(|answered| match answered {
            Ok(_) => "sent".to_owned(),
            Err(ResilienceError::Limited { retry_after, .. }) => format!("limited {retry_after:?}"),
            Err(other) => panic!("{other:?}"),
        });
        answered.sort();
        assert_eq!(answered, ["limited 10s", "sent"]);
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(10),
            "the one refused again waited no second time"
        );
    }

    /// The waits of one attempt end by its first refusal plus its wait, so a
    /// timer firing late counts against the wait too: refused at 0 s with 21 s
    /// to wait, woken at 12 s for the reset at 10 s, and refused again by the
    /// window another call opened then, it does not wait for the reset at
    /// 22 s, though the two waits it was told add up to 20 s.
    #[tokio::test(start_paused = true)]
    async fn a_wait_whose_timer_fires_late_counts_whole_against_the_attempts_wait() {
        let policy = policy(rated(
            1,
            Duration::from_secs(10),
            Some(Duration::from_secs(21)),
        ));
        assert!(policy.run(|_| answering()).await.is_ok());
        let mut waiting = Box::pin(policy.run(|_| answering()));
        park(&mut waiting).await;
        tokio::time::advance(Duration::from_secs(12)).await;
        assert!(
            policy.run(|_| answering()).await.is_ok(),
            "a call opens the next window at 12 s"
        );
        let refused = waiting.await;
        assert!(
            matches!(refused, Err(ResilienceError::Limited { retry_after, .. })
                if retry_after == Duration::from_secs(10)),
            "the reset at 22 s, past the 21 s its wait allows: {refused:?}"
        );
    }

    /// An attempt refused again once its wait ends waits again while what is
    /// left of its wait reaches the next reset: refused at 0 s with 25 s to
    /// wait, woken at 10 s, and refused by the window another call opened
    /// then, it waits for the reset at 20 s and is sent there.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_refused_again_after_its_wait_waits_again_for_a_reset_its_wait_reaches() {
        let policy = policy(rated(
            1,
            Duration::from_secs(10),
            Some(Duration::from_secs(25)),
        ));
        let calls = AtomicU32::new(0);
        let sent = || async {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok::<_, Refused>(())
        };
        assert!(policy.run(|_| sent()).await.is_ok());
        let started = tokio::time::Instant::now();
        let mut waiting = Box::pin(policy.run(|_| sent()));
        park(&mut waiting).await;
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(
            policy.run(|_| sent()).await.is_ok(),
            "a call opens the next window at 10 s"
        );
        let answered = waiting.await;
        assert!(matches!(answered, Ok(())), "{answered:?}");
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(20),
            "sent at the reset after its second wait"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    /// An attempt whose clock was read before another call opened the window —
    /// a call reading the clock later, on another worker, counted first — is
    /// told the time from its own read to that window's reset, so the wait it
    /// computes ends at the reset: with no wait it is refused for all of it, and
    /// with a wait of the window's length, which that reset lies past, it is
    /// refused at once rather than woken before the reset and refused again.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_read_before_another_opened_the_window_is_told_that_windows_reset() {
        let window = Duration::from_secs(10);
        let later = Duration::from_millis(5);
        for wait in [None, Some(window)] {
            let policy = policy(rated(1, window, wait));
            let started = tokio::time::Instant::now();
            let rate = policy.rate.as_ref().expect("a rate limit");
            assert_eq!(
                rate.count(started + later),
                Ok(()),
                "the other call opens it"
            );
            let Err(refused) = policy.run(|_| answering()).await else {
                panic!("with {wait:?} to wait, the window is spent");
            };
            assert!(
                matches!(refused, ResilienceError::Limited { retry_after, .. }
                    if retry_after == window + later),
                "with {wait:?} to wait, told the reset {later:?} past the window: {refused:?}"
            );
            assert_eq!(
                started.elapsed(),
                Duration::ZERO,
                "with {wait:?}, refused at once"
            );
            let problem = nest_rs_core::__private::find_in_chain::<Problem>(&refused)
                .expect("the problem rides the chain");
            assert_eq!(problem.retry_after(), Some(11), "its reset rounded up");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_bulkhead_slot_stays_held_while_an_attempt_waits_for_the_reset() {
        let policy = policy(ResilienceConfig {
            concurrency: Some(1),
            ..rated(1, Duration::from_secs(10), Some(Duration::from_secs(10)))
        });
        assert!(policy.run(|_| answering()).await.is_ok());
        let (waited, refused) = tokio::join!(policy.run(|_| answering()), async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            policy.run(|_| answering()).await
        });
        assert!(matches!(waited, Ok(())), "{waited:?}");
        assert!(
            matches!(refused, Err(ResilienceError::Full { .. })),
            "the call waiting for the reset holds the one slot: {refused:?}"
        );
    }

    /// A call finding no free slot reads the rate before it waits for one, as
    /// `begin` reads the breaker: a window spent whose reset lies past the
    /// rate's wait, or at the unit's deadline, refuses it at once, told that
    /// reset, rather than once its wait for a slot ends. The read counts
    /// nothing: a call waiting for its slot beside a window with room is sent
    /// once the slot frees, as the window's second attempt.
    #[tokio::test(start_paused = true)]
    async fn a_call_finding_no_free_slot_is_refused_by_a_spent_rate_before_it_waits() {
        let lines = nest_rs_testing::LogCapture::install();
        let tracking =
            crate::resilient::resilient_method(|| "ShipmentsService", "track", None, None);
        for (rate_wait, left, past) in [
            (Duration::from_secs(5), None, "the rate's 5 s wait"),
            (
                Duration::from_secs(60),
                Some(Duration::from_secs(30)),
                "the unit's deadline 30 s away",
            ),
        ] {
            let policy = policy(ResilienceConfig {
                concurrency: Some(1),
                concurrency_wait: Some(Duration::from_secs(60)),
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
                "the window's first attempt, holding the slot 3 s: {holding:?}"
            );
            assert!(
                matches!(waited, (at, Ok(())) if at == Duration::from_secs(3)),
                "the window's second attempt, sent once the slot freed: {waited:?}"
            );

            let held = policy.begin(None).await.expect("the slot, free again");
            let refused_at = tokio::time::Instant::now();
            let refused = nest_rs_core::__private::with_deadline(
                left.map(|left| refused_at + left),
                policy.run_once::<Ran, _, _>(Some(tracking), |_| answering()),
            )
            .await;
            assert!(
                matches!(refused, Err(ResilienceError::Limited { retry_after, .. })
                    if retry_after == Duration::from_secs(57)),
                "the reset 57 s away, past {past}: {refused:?}"
            );
            assert_eq!(
                refused_at.elapsed(),
                Duration::ZERO,
                "refused at once, rather than after its wait for the slot held"
            );
            drop(held);
        }

        let limited = lines.find(TARGET, "rate limit reached; the call is refused");
        assert_eq!(limited.len(), 2, "{limited:#?}");
        for line in &limited {
            assert_eq!(line.level, "debug", "{line:?}");
            assert_eq!(line.field("instance").as_deref(), Some("upstream"));
            assert_eq!(line.field("host").as_deref(), Some("ShipmentsService"));
            assert_eq!(line.field("method").as_deref(), Some("track"));
            assert_eq!(line.field("retry_after_ms").as_deref(), Some("57000"));
        }
        assert!(
            lines
                .find(TARGET, "bulkhead full; the call is refused")
                .is_empty(),
            "no call met the full bulkhead"
        );
    }

    /// The rate a call finding no free slot reads comes after the unit's
    /// deadline and the breaker, in an attempt's order: beside the slot held
    /// and the window spent, a unit out of time is told so, and a call meeting
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
            "the window's one attempt"
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
            lines
                .find(TARGET, "rate limit reached; the call is refused")
                .is_empty(),
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
    /// probe's call out, or waiting for the rate's reset with its slot held —
    /// is refused at once, told what is left of that probe's bound, rather than
    /// after a wait for the slot the probe holds.
    #[tokio::test(start_paused = true)]
    async fn a_call_finding_no_free_slot_beside_the_probe_is_refused_at_once_told_its_bound() {
        for (rate_limit, told, probe) in [
            (None, 4, "its call out, 1 s into its 5 s timeout"),
            (
                Some(1),
                13,
                "waiting for the rate's reset 8 s away, then for its 5 s timeout",
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
    /// the reset; the unit's deadline bounds the wait and the attempt together.
    #[tokio::test(start_paused = true)]
    async fn the_wait_for_the_reset_is_none_of_the_attempts_and_the_units_deadline_bounds_both() {
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

    /// A wait for the reset is taken only when the reset comes before the
    /// unit's deadline, but its timer may fire past it: the attempt is then
    /// refused as a unit out of time, unsent, and takes nothing of the window.
    /// Its call never answers, so what was sent alone tells that refusal from
    /// a call sent with no time left.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_whose_wait_for_the_reset_ends_past_the_units_deadline_is_refused_unsent() {
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
            "its wait for the reset at 10 s ended at 12 s, past its deadline at 11 s: {late:?}"
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
            "the next call opens the window at once"
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
        assert!(!run.again(&limited).await);
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

    /// A call dropped while it waits for the reset leaves nothing behind: the
    /// next call takes its slot, and probes in its turn — a probe, since its
    /// failure opens the breaker again where a closed one would hold two.
    #[tokio::test(start_paused = true)]
    async fn a_caller_dropped_while_it_waits_frees_its_slot_and_hands_its_probe_on() {
        let policy = policy(ResilienceConfig {
            concurrency: Some(1),
            breaker_window: Some(2),
            breaker_failures: 2,
            breaker_open: Duration::from_secs(30),
            ..rated(2, Duration::from_secs(60), Some(Duration::from_secs(60)))
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
            "it waited for the reset"
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

    /// A run dropped while its probe waits for the rate's reset hands the
    /// probe's turn on before its slot of the bulkhead too: the call that slot
    /// wakes takes the turn, waits for that reset in the dropped probe's place,
    /// and closes the breaker once it is sent.
    #[tokio::test(start_paused = true)]
    async fn a_run_dropped_while_its_probe_waits_for_the_reset_hands_the_turn_on_before_its_slot() {
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
        let mut probing = Box::pin(policy.run(|_| answering()));
        park(&mut probing).await;
        let waiting = PollsOnWake::new(policy.run(|_| async { Ok::<_, Refused>("sent") }));
        waiting.poll_now();
        assert!(
            matches!(failing.await, Err(ResilienceError::Failed(Refused))),
            "the rate's one attempt in its window fails, opening the breaker for a second"
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        park(&mut probing).await;
        assert!(
            waiting.answered().is_none(),
            "it waits for the slot the probe holds while it waits for the reset"
        );

        drop(probing);
        let answer = waiting.answered();
        assert!(
            answer.is_none(),
            "woken by the slot the dropped run freed, it took the turn its probe handed on and \
             waits for the reset: {answer:?}"
        );
        tokio::time::sleep(Duration::from_secs(60)).await;
        let answer = waiting.answered();
        assert!(
            matches!(answer, Some(Ok("sent"))),
            "sent as the probe at the reset: {answer:?}"
        );
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Closed),
            "and closed the breaker"
        );
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
            .find(TARGET, "circuit breaker changed state")
            .pop()
            .expect("the probe's transition");
        assert_eq!(
            (reopened.field("state"), reopened.field("failed")),
            (Some("open".to_owned()), Some("1".to_owned())),
            "filed as a failed probe: {reopened:?}"
        );
    }

    /// A call refused while the probe waits for the rate's reset is told when
    /// the probe has answered at the latest: what is left of that wait, then
    /// its call's timeout.
    #[tokio::test(start_paused = true)]
    async fn a_call_refused_while_the_probe_waits_for_the_reset_is_told_the_reset_then_its_timeout()
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
            "at 31 s, the probe waiting for the reset at 60 s, then 2 s: {refused:?}"
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

    /// Once the probe's call is sent no wait for the reset is left in its
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
            policy.run_once::<Ran, _, _>(Some(tracking), |_| async {
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

    /// A probe holding its turn while it waits for the rate's reset is waited
    /// for until that reset, then for its own bound: the timeout its method
    /// pinned, or what its unit's deadline leaves of the policy's.
    #[tokio::test(start_paused = true)]
    async fn a_probe_waiting_for_the_reset_is_waited_for_until_it_then_for_its_own_bound() {
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
                    policy.run_once::<Ran, _, _>(method, |_| answering())
                ),
                async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    policy.run(|_| answering()).await
                },
            );
            assert!(matches!(probed, Ok(())), "sent at the reset: {probed:?}");
            assert!(
                matches!(refused, Err(ResilienceError::Open { retry_after, .. })
                    if retry_after == Duration::from_secs(told)),
                "at 2 s, the probe sent at the reset at 10 s, under {bound}: {refused:?}"
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
    /// for the reset ends: a breaker another call opened meanwhile refuses it
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
            "refused at the reset it waited for"
        );
        assert_eq!(
            sent.load(Ordering::Relaxed),
            1,
            "the attempt refused at the reset is never sent"
        );
    }

    /// An attempt waiting for the reset while another call took the probe's
    /// turn is refused when its wait ends: one call probes the dependency at a
    /// time, and every other is told when it has answered at the latest.
    #[tokio::test(start_paused = true)]
    async fn an_attempt_waiting_for_the_reset_is_never_sent_beside_the_probe() {
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
                if retry_after == Duration::from_secs(5)),
            "the probe sent at the reset, within its 5 s timeout: {beside:?}"
        );
        assert_eq!(
            *sent.lock().expect("the calls sent"),
            ["opening", "filling", "probe"]
        );
    }

    /// An attempt whose breaker opened, and whose open time passed, while it
    /// waited for the reset is the probe when its wait ends, so its answer
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
            .find(TARGET, "circuit breaker changed state")
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
    /// caller dropped while it waits for the reset — files no transition: the
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
                .find(TARGET, "circuit breaker changed state")
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
                "a reset 59 s away, past the 30 s wait: {refused:?}"
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
            "the probe sent at the reset"
        );
    }

    /// A probe whose wait for the reset ends past its unit's deadline is given
    /// up unsent too, as a unit out of time: it files no transition, takes
    /// nothing of the window, and the next call probes in its turn.
    #[tokio::test(start_paused = true)]
    async fn a_probe_whose_wait_for_the_reset_ends_past_the_units_deadline_hands_its_turn_on() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(ResilienceConfig {
            breaker_window: Some(1),
            breaker_failures: 1,
            breaker_open: Duration::from_secs(1),
            ..rated(1, Duration::from_secs(10), Some(Duration::from_secs(10)))
        });
        let states = || -> Vec<String> {
            lines
                .find(TARGET, "circuit breaker changed state")
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
            "its wait for the reset at 10 s ended at 12 s, past its deadline at 11 s: {late:?}"
        );
        assert_eq!(states(), ["open"], "the probe given up, never sent");

        let resumed = tokio::time::Instant::now();
        assert!(policy.run(|_| answering()).await.is_ok());
        assert_eq!(
            resumed.elapsed(),
            Duration::ZERO,
            "the next call opens the window at once"
        );
        assert_eq!(
            states(),
            ["open", "half_open", "closed"],
            "the next call probed"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_refusal_by_the_rate_and_a_wait_for_its_reset_are_debug_lines_naming_the_instance() {
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

        let refusal = lines.expect_one(TARGET, "rate limit reached; the call is refused");
        let wait = lines.expect_one(
            TARGET,
            "rate limit reached; waiting for the window to reset",
        );
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

    /// A reset under a millisecond away is filed in whole milliseconds rounded
    /// up, `1`, never the `0` of no wait: told an attempt the rate refuses, and
    /// waited for by one whose wait reaches it, which is sent once the reset
    /// has passed, never before.
    #[tokio::test(start_paused = true)]
    async fn a_reset_under_a_millisecond_away_is_filed_as_one_never_as_zero() {
        let lines = nest_rs_testing::LogCapture::install();
        let window = Duration::from_secs(1);
        let refusing = policy(rated(1, window, None));
        let waiting = policy(rated(1, window, Some(Duration::from_millis(1))));
        for policy in [&refusing, &waiting] {
            assert!(policy.run(|_| answering()).await.is_ok(), "opens a window");
        }
        let near = Duration::from_micros(500);
        tokio::time::advance(window - near).await;
        let refused = refusing.run(|_| answering()).await;
        assert!(
            matches!(refused, Err(ResilienceError::Limited { retry_after, .. }) if retry_after == near),
            "{refused:?}"
        );
        let reset = tokio::time::Instant::now() + near;
        let sent = std::cell::Cell::new(None);
        let answered = waiting
            .run(|_| async {
                sent.set(Some(tokio::time::Instant::now()));
                Ok::<_, Refused>(())
            })
            .await;
        assert!(answered.is_ok(), "{answered:?}");
        assert!(
            sent.get().is_some_and(|sent| sent >= reset),
            "sent once the reset has passed: {:?}",
            sent.get()
        );

        let refusal = lines.expect_one(TARGET, "rate limit reached; the call is refused");
        let wait = lines.expect_one(
            TARGET,
            "rate limit reached; waiting for the window to reset",
        );
        for (line, field) in [(&refusal, "retry_after_ms"), (&wait, "wait_ms")] {
            assert_eq!(line.field(field).as_deref(), Some("1"), "{line:?}");
        }
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
            "its retry, refused unsent by the window its first attempt filled: {limited:?}"
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

    /// Under a rate limit, a breaker opening on more failures than can answer
    /// within its age of one another never opens, and is refused naming the
    /// settings that decide it, quoting none of their values. Three attempts
    /// an hour let five answer within a minute at most — the two after a
    /// window's first, sent as it ends, and three as the next one opens — and
    /// a breaker opening on five is built, and opens on them.
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
        let refused = ResiliencePolicy::<Upstream>::new(&config(6))
            .expect_err("a breaker that never opens")
            .to_string();
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

        let policy = policy(config(5));
        let failed = || async {
            let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
            assert!(
                matches!(failed, Err(ResilienceError::Failed(Refused))),
                "{failed:?}"
            );
        };
        assert!(policy.run(|_| answering()).await.is_ok(), "the first");
        tokio::time::advance(Duration::from_secs(3599)).await;
        for _ in 0..2 {
            failed().await;
        }
        tokio::time::advance(Duration::from_secs(1)).await;
        for _ in 0..3 {
            failed().await;
        }
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "five failures within a second of one another"
        );
    }

    /// Where the age and the longest attempt make a whole number of the rate's
    /// windows, a window open as that span starts leaves no room for one
    /// opening at its end, so the most failures that answer within the age
    /// are a full window's at each opening. Three attempts every ten minutes,
    /// with a ten minutes' age, let nine: three cut at the ten minutes an
    /// attempt takes at most, three sent as they are cut, as the next window
    /// opens, and three as the one after opens, the age later. A breaker
    /// opening on nine is built and opens on them; one opening on ten is
    /// refused.
    #[tokio::test(start_paused = true)]
    async fn more_failures_than_the_windows_opening_in_a_span_of_whole_windows_send_is_refused() {
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
            "a breaker that never opens"
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
        assert!(
            lines
                .find(TARGET, "circuit breaker changed state")
                .is_empty()
        );
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
        assert!(
            lines
                .find(TARGET, "circuit breaker changed state")
                .is_empty()
        );
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

    /// A window under a second, set in code, sends its limit within it and
    /// refuses the next attempt unsent, told the time to its reset in a
    /// `Retry-After` of a second, rounded up; past the reset, it sends again.
    #[tokio::test(start_paused = true)]
    async fn a_rate_window_under_a_second_resets_that_often() {
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
            window - into,
            "the window the first call opened resets at 500 ms"
        );
        let problem = nest_rs_core::__private::find_in_chain::<Problem>(&refused)
            .expect("the problem rides the chain");
        assert_eq!(problem.retry_after(), Some(1), "a second, rounded up");
        assert_eq!(
            calls.load(Ordering::Relaxed),
            2,
            "a refused call is never sent"
        );
        tokio::time::advance(window - into).await;
        assert!(
            policy.run(|_| sent()).await.is_ok(),
            "the next window opens"
        );
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

    /// A probe holding its turn while it waits for the rate's reset reads
    /// half-open, carrying no time, while a call beside it is told when the
    /// probe has answered at the latest. Opened by hand meanwhile, the breaker
    /// refuses the probe when its wait ends, unsent, as it refuses any call
    /// meeting it open, before the rate counts it.
    #[tokio::test(start_paused = true)]
    async fn a_probe_waiting_for_the_reset_when_a_manual_opening_comes_is_refused_unsent() {
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
            "at 1 s, the probe sent at the reset at 10 s, within its 5 s timeout: {beside:?}"
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
            "the refused probe took nothing of the window after its wait"
        );
    }

    /// A probe waiting for the rate's reset loses its turn to a manual
    /// opening, which may pass before the wait ends: the call then meets the
    /// breaker as a call arriving then does, takes the probe's turn of the
    /// opening by hand, is sent, and its answer closes the breaker.
    #[tokio::test(start_paused = true)]
    async fn a_probe_waiting_for_the_reset_past_a_shorter_manual_opening_probes_after_it() {
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
        assert_eq!(sent.load(Ordering::Relaxed), 1, "sent at the reset");
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
        let states: Vec<String> = lines
            .find(TARGET, "circuit breaker changed state")
            .iter()
            .map(|line| line.field("state").unwrap_or_default())
            .collect();
        assert_eq!(
            states,
            ["open", "open", "half_open", "closed"],
            "opened by a failure, then by hand, then probed at the reset"
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
}
