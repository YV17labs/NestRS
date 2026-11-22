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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use nest_rs_config::{ConfigError, Namespaced, instance_namespace};
use nest_rs_core::{
    Backoff, Deadline, Instance, Problem, Unanswered, Unnamed, process_seed, within,
};
use tokio::sync::SemaphorePermit;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::breaker::{Breaker, BreakerState, Pass};
use crate::bulkhead::Bulkhead;
use crate::config::{BREAKER_OPEN_TIMES, LONGEST_ATTEMPT, ResilienceConfig};
use crate::counters::Counters;
use crate::error::{BreakerError, Refusal, ResilienceError};
use crate::rate::Rate;
use crate::resilient::ResilientMethod;
use crate::{TARGET, millis};

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
    policy: &'p ResiliencePolicy<I>,
    _slot: Option<SemaphorePermit<'p>>,
    /// The deadline of the unit the run is in, which no attempt and no wait
    /// outlives.
    deadline: Option<Instant>,
    /// How long each attempt waits: the policy's timeout, or the one its
    /// method pinned.
    timeout: Duration,
    /// How many times the run may send its call again: the policy's retries,
    /// or the ones its method pinned.
    retries: u32,
    /// The decorated method the run came from, which its lines name.
    method: Option<ResilientMethod>,
    /// Drawn at the first retry, so a call answering at once draws none.
    seed: Option<u64>,
    /// How many times this run attempted its call again so far, a retry the
    /// policy refused unsent included.
    pub(crate) resent: u32,
}

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
            bulkhead: config
                .concurrency
                .map(|slots| Bulkhead::new(slots, config.concurrency_wait)),
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
    pub async fn run<T, E, F, Fut>(&self, call: F) -> Result<T, ResilienceError<E>>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        self.run_for(None, call).await
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
    pub async fn run_idempotent<T, E, F, Fut>(&self, call: F) -> Result<T, ResilienceError<E>>
    where
        F: FnMut(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: Error + 'static,
    {
        self.run_idempotent_for(None, call).await
    }

    /// [`run`](Self::run), for `method` when a decorated method runs it.
    pub(crate) async fn run_for<T, E, F, Fut>(
        &self,
        method: Option<ResilientMethod>,
        call: F,
    ) -> Result<T, ResilienceError<E>>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let mut run = self.begin(method).await?;
        let attempted = run.attempt(call, judged).await;
        settled(run.finish(attempted))
    }

    /// [`run_idempotent`](Self::run_idempotent), for `method` when a decorated
    /// method runs it.
    pub(crate) async fn run_idempotent_for<T, E, F, Fut>(
        &self,
        method: Option<ResilientMethod>,
        mut call: F,
    ) -> Result<T, ResilienceError<E>>
    where
        F: FnMut(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: Error + 'static,
    {
        let mut run = self.begin(method).await?;
        loop {
            let attempted = run.attempt(&mut call, judged_asking).await;
            if !run.again(&attempted).await {
                return settled(run.finish(attempted));
            }
        }
    }

    /// Open one run — of `method`, when a decorated method opens it — taking a
    /// slot of the bulkhead within the unit's deadline, refused at once, in an
    /// attempt's order, by a unit out of time and then by an open breaker,
    /// rather than after waiting for one.
    pub(crate) async fn begin(
        &self,
        method: Option<ResilientMethod>,
    ) -> Result<Run<'_, I>, Refusal> {
        let deadline = nest_rs_core::ambient::<Deadline>().map(|deadline| deadline.0);
        let slot = match &self.bulkhead {
            Some(bulkhead) => {
                if deadline.is_some_and(|deadline| deadline <= Instant::now()) {
                    return Err(self.refused(Refusal::TimedOut(Duration::ZERO)));
                }
                if let Some(retry_after) = self.breaker.as_ref().and_then(Breaker::refusing) {
                    return Err(self.refused(Refusal::Open(retry_after)));
                }
                let Some(slot) = bulkhead.enter(deadline).await else {
                    tracing::debug!(
                        target: TARGET,
                        instance = I::NAME,
                        host = method.as_ref().map(ResilientMethod::host).as_deref(),
                        method = method.as_ref().map(ResilientMethod::name),
                        "bulkhead full; the call is refused",
                    );
                    return Err(self.refused(Refusal::Full));
                };
                Some(slot)
            }
            None => None,
        };
        Ok(Run {
            policy: self,
            _slot: slot,
            deadline,
            timeout: method
                .and_then(|method| method.timeout())
                .unwrap_or(self.timeout),
            retries: method
                .and_then(|method| method.retries())
                .unwrap_or(self.retries),
            method,
            seed: None,
            resent: 0,
        })
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
    /// [`breaker_open`](ResilienceConfig::breaker_open) takes: every call is
    /// refused unsent, as by an open breaker, told what is left of it, then
    /// the breaker probes the dependency as after any opening.
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
    /// take; either leaves the breaker as it was.
    pub fn open_breaker(&self, duration: Duration) -> Result<(), BreakerError> {
        let breaker = self.breaker.as_ref().ok_or(BreakerError::Off)?;
        if !BREAKER_OPEN_TIMES.contains(&duration) {
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

    /// One attempt of `call`, let through by the unit's deadline, the breaker
    /// and the rate, then bounded by the run's timeout or what is left of the
    /// unit's deadline, whichever ends first, its answer counted as `judge`
    /// reads it. A retry counts on the retries' counter when it is let
    /// through, its call sent. A panic in the call counts as a failure and
    /// unwinds on, for the unit to contain as it would without the policy.
    pub(crate) async fn attempt<T, F, Fut>(
        &mut self,
        call: F,
        judge: impl FnOnce(&T) -> Verdict,
    ) -> Attempted<T>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = T>,
    {
        let (pass, bound) = self.admitted().await?;
        if self.resent > 0 {
            self.policy.counters.retry();
        }
        let sent = pass.map(|pass| pass.send(bound));
        let cancel = CancellationToken::new();
        let given_up = cancel.clone().drop_guard();
        let answered = within(bound, async move { call(cancel).await })
            .await
            .map(|answer| {
                let verdict = judge(&answer);
                (answer, verdict)
            });
        if let Some(sent) = sent {
            sent.settle(!matches!(answered, Ok((_, Verdict::Succeeded))));
        }
        match answered {
            Ok(judged) => {
                let _kept = given_up.disarm();
                Ok(judged)
            }
            Err(Unanswered::Panicked(payload)) => std::panic::resume_unwind(payload),
            Err(_elapsed) => Err(Refusal::TimedOut(bound)),
        }
    }

    /// How long an attempt sent now may run: the run's timeout, or what is left
    /// of the unit's deadline when that is shorter.
    fn bound(&self) -> Duration {
        if self.deadline.is_none() {
            return self.timeout;
        }
        self.bound_at(Instant::now())
    }

    /// How long an attempt sent at `at` may run.
    fn bound_at(&self, at: Instant) -> Duration {
        self.deadline.map_or(self.timeout, |deadline| {
            self.timeout.min(deadline.saturating_duration_since(at))
        })
    }

    /// The attempt let through, in order, by what is left of the unit's
    /// deadline, the breaker and the rate, with the bound its call is sent
    /// under; with nothing left of the deadline it is not sent, nor counted.
    /// Past the rate's limit it waits for the window's reset when what is left
    /// of the rate's wait reaches it before the unit's deadline — a wait that
    /// is no part of the attempt — then goes through all three again, a probe
    /// keeping its turn and telling the breaker when it has answered at the
    /// latest; otherwise it is refused, counted for nothing, told the time to
    /// that reset.
    async fn admitted(&self) -> Result<(Option<Pass<'p>>, Duration), Refusal> {
        let policy = self.policy;
        let mut pass: Option<Pass<'p>> = None;
        let mut waits_until = None;
        loop {
            let bound = self.bound();
            if bound.is_zero() {
                return Err(Refusal::TimedOut(bound));
            }
            // Another call may have opened the breaker, or taken the probe's
            // turn, while this one waited for the reset, and a manual switch
            // meanwhile voids the turn this one held.
            if !pass.as_ref().is_some_and(Pass::probes) {
                pass = policy
                    .breaker
                    .as_ref()
                    .map(|breaker| breaker.admit(bound))
                    .transpose()
                    .map_err(Refusal::Open)?;
            }
            let Some(rate) = &policy.rate else {
                return Ok((pass, bound));
            };
            let now = Instant::now();
            let Err(reset) = rate.count(now) else {
                return Ok((pass, bound));
            };
            let until = *waits_until.get_or_insert_with(|| now + rate.wait());
            let at = now + reset;
            if at > until || self.deadline.is_some_and(|deadline| at >= deadline) {
                tracing::debug!(
                    target: TARGET,
                    instance = I::NAME,
                    host = self.host().as_deref(),
                    method = self.method_name(),
                    retry_after_ms = millis(reset),
                    "rate limit reached; the call is refused",
                );
                return Err(Refusal::Limited(reset));
            }
            if let Some(pass) = &pass {
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
            tokio::time::sleep_until(at).await;
        }
    }

    /// The run's last attempt, its refusal counted when it ended on one.
    pub(crate) fn finish<T>(&self, attempted: Attempted<T>) -> Attempted<T> {
        attempted.map_err(|refusal| self.policy.refused(refusal))
    }

    /// Whether to attempt the call again after `attempted`, having waited the
    /// backoff when so: after a failure or a timeout, within the run's
    /// retries, unless the dependency asked a wait past the backoff's ceiling.
    ///
    /// It is decided before the wait, which holds nothing of the answer, so a
    /// run is `Send` for an answer that is `Send` and not `Sync`, a stream's.
    pub(crate) fn again<'r, T>(
        &'r mut self,
        attempted: &Attempted<T>,
    ) -> impl Future<Output = bool> + use<'r, 'p, I, T> {
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
                host = self.host().as_deref(),
                method = self.method_name(),
                attempt = self.resent.saturating_add(1),
                wait_ms = millis(wait),
                "attempt failed; retrying the call",
            );
        }
        async move {
            let Some(wait) = wait else {
                return false;
            };
            tokio::time::sleep(wait).await;
            self.resent = self.resent.saturating_add(1);
            true
        }
    }

    /// How long to wait before the next attempt, after one that failed asking
    /// `asked`, or `None` when the run ends there.
    fn retry_wait(&mut self, asked: Option<Duration>) -> Option<Duration> {
        let policy = self.policy;
        if self.resent >= self.retries {
            return None;
        }
        let ceiling = policy.backoff.ceiling();
        if let Some(asked) = asked.filter(|asked| *asked > ceiling) {
            tracing::debug!(
                target: TARGET,
                instance = I::NAME,
                host = self.host().as_deref(),
                method = self.method_name(),
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
        if let Some(deadline) = self.deadline
            && Instant::now() + wait >= deadline
        {
            tracing::debug!(
                target: TARGET,
                instance = I::NAME,
                host = self.host().as_deref(),
                method = self.method_name(),
                wait_ms = millis(wait),
                "no time left before the unit's deadline; not attempted again",
            );
            return None;
        }
        Some(wait)
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
            concurrency_wait: Duration::from_secs(60),
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
            .send(policy.timeout)
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
            concurrency_wait: Duration::from_secs(10),
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

    /// No retry and no breaker, `limit` attempts per `window`, each attempt past
    /// it waiting up to `wait` for the reset: the subject is the rate.
    fn rated(limit: usize, window: Duration, wait: Duration) -> ResilienceConfig {
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
            ..rated(3, Duration::from_secs(60), Duration::ZERO)
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
        let policy = policy(rated(2, Duration::from_secs(60), Duration::ZERO));
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
        let policy = policy(rated(1, Duration::from_secs(60), Duration::ZERO));
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
            ..rated(2, Duration::from_secs(60), Duration::ZERO)
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
            ..rated(1, Duration::from_secs(60), Duration::ZERO)
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
            ..rated(2, Duration::from_secs(60), Duration::ZERO)
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
        let policy = policy(rated(1, Duration::from_secs(10), Duration::from_secs(10)));
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
        let policy = policy(rated(1, Duration::from_secs(10), Duration::from_secs(5)));
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
        let policy = policy(rated(1, Duration::from_secs(10), Duration::from_secs(15)));
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
        let policy = policy(rated(1, Duration::from_secs(10), Duration::from_secs(21)));
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

    #[tokio::test(start_paused = true)]
    async fn the_bulkhead_slot_stays_held_while_an_attempt_waits_for_the_reset() {
        let policy = policy(ResilienceConfig {
            concurrency: Some(1),
            ..rated(1, Duration::from_secs(10), Duration::from_secs(10))
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

    /// The attempt's timeout starts when its call is sent, after the wait for
    /// the reset; the unit's deadline bounds the wait and the attempt together.
    #[tokio::test(start_paused = true)]
    async fn the_wait_for_the_reset_is_none_of_the_attempts_and_the_units_deadline_bounds_both() {
        let policy = policy(ResilienceConfig {
            timeout: Duration::from_secs(1),
            ..rated(1, Duration::from_secs(5), Duration::from_secs(10))
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
        let policy = policy(rated(1, Duration::from_secs(10), Duration::from_secs(10)));
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
            ..rated(1, Duration::from_secs(60), Duration::ZERO)
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
            ..rated(1, Duration::from_secs(1), Duration::from_secs(60))
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
            ..rated(2, Duration::from_secs(60), Duration::from_secs(60))
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
            ..rated(1, Duration::from_secs(60), Duration::from_secs(40))
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
            rate_wait: Duration::from_secs(60),
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
            policy.run_for(Some(tracking), |_| async {
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
                rate_wait: Duration::from_secs(10),
                ..probing(Duration::from_secs(5))
            });
            probe_due(&policy).await;
            let deadline = left.map(|left| tokio::time::Instant::now() + left);
            let (probed, refused) = tokio::join!(
                nest_rs_core::__private::with_deadline(
                    deadline,
                    policy.run_for(method, |_| answering())
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
            ..rated(1, Duration::from_secs(10), Duration::from_secs(10))
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
            ..rated(2, Duration::from_secs(10), Duration::from_secs(10))
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
            ..rated(1, Duration::from_secs(10), Duration::from_secs(10))
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
            ..rated(1, Duration::from_secs(60), Duration::from_secs(30))
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
            ..rated(1, Duration::from_secs(10), Duration::from_secs(10))
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
        let policy = policy(rated(1, Duration::from_secs(10), Duration::from_secs(5)));
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

    /// Each retry sent adds one to the retries' counter, and each run ending on
    /// the policy's refusal one to the refusals', by that refusal: a retry the
    /// breaker or the rate refuses, never sent, adds to the refusals' alone.
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
                ("full".to_owned(), 1),
                ("limited".to_owned(), 1),
                ("open".to_owned(), 1),
                ("timed_out".to_owned(), 1),
            ],
            "a failed call is the call's answer, no refusal of the policy's"
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

    /// A manual opening for a duration `breaker_open` would not take is
    /// refused, never clamped: the breaker stays as it was and files nothing,
    /// and the refusal names the open times it takes and their variable's key,
    /// never the duration given. Both ends are taken.
    #[tokio::test(start_paused = true)]
    async fn a_manual_opening_outside_the_open_times_breaker_open_takes_is_refused_unclamped() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(ResilienceConfig::default());
        let bounds = ResilienceConfig::BREAKER_OPEN.bounds();
        let nest_rs_config::Floor::Units(floor) = bounds.least() else {
            panic!("the open time's floor is a count of seconds");
        };
        let least = bounds.unit().duration(floor.count);
        let most = bounds.unit().duration(bounds.most().count);
        let tick = Duration::from_millis(1);
        for outside in [Duration::ZERO, least - tick, most + tick, Duration::MAX] {
            let refused = policy
                .open_breaker(outside)
                .expect_err("outside the open times");
            assert_eq!(refused, BreakerError::OutOfRange { least, most });
            let said = refused.to_string();
            assert_eq!(
                said,
                format!(
                    "a breaker opened by hand stays open {least:?} to {most:?}, the open times \
                     `{}` takes",
                    ResilienceConfig::BREAKER_OPEN.key()
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
        for end in [least, most] {
            assert_eq!(policy.open_breaker(end), Ok(()));
            assert_eq!(
                policy.breaker_state(),
                Some(BreakerState::Open { retry_after: end })
            );
        }
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
            ..rated(1, Duration::from_secs(10), Duration::from_secs(10))
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
            ..rated(1, Duration::from_secs(10), Duration::from_secs(10))
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
