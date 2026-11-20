//! [`ResiliencePolicy`] — one named policy around the calls to one dependency:
//! a timeout per attempt, retries for an idempotent call, a circuit breaker and
//! a bulkhead, every one of them shared by every caller in the process.

use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use nest_rs_config::{ConfigError, instance_namespace};
use nest_rs_core::{Backoff, Instance, Unanswered, Unnamed, process_seed, within};
use tokio::sync::SemaphorePermit;
use tokio_util::sync::CancellationToken;

use crate::TARGET;
use crate::breaker::{Admission, Breaker};
use crate::bulkhead::Bulkhead;
use crate::config::ResilienceConfig;
use crate::error::{Refusal, ResilienceError};

/// The calls to one dependency — a payment provider, a carrier's API, a
/// language model — run under one policy, by every caller in the process.
///
/// Injected as `Arc<ResiliencePolicy<I>>` once a module imports
/// [`ResilienceModule::<I>`](crate::ResilienceModule), which builds it from
/// `<PREFIX>_RESILIENCE__<INSTANCE>__*` ([`ResilienceConfig`]); the default
/// instance, `ResiliencePolicy`, reads `<PREFIX>_RESILIENCE__*`.
///
/// Each attempt is given up past the policy's timeout, and its
/// [`CancellationToken`] cancelled, so work the call handed to another task
/// stops with it. A call is attempted again only when it is declared
/// idempotent, through [`run_idempotent`](Self::run_idempotent): replaying one
/// that is not would charge a card twice. Every attempt's outcome counts in the
/// circuit breaker, which refuses every call at once while it is open, and the
/// bulkhead bounds how many calls run inside the policy together.
///
/// Every `Err` the call answers counts as the dependency failing. An answer
/// the dependency gave — a declined card, a parcel not found — belongs in the
/// call's `Ok`, so it is neither retried nor counted.
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
/// #[module(imports = [ResilienceModule::<Upstream>], providers = [PreviewService])]
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
    /// Sets each run's jitter apart from the others'.
    runs: AtomicU64,
    _instance: PhantomData<fn() -> I>,
}

/// How an attempt's answer counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The dependency answered.
    Succeeded,
    /// The dependency failed, asking the wait before a retry when it said one.
    Failed { asked: Option<Duration> },
}

/// One attempt's answer and how it counts, or the policy's refusal.
pub(crate) type Attempted<T> = Result<(T, Verdict), Refusal>;

/// One run of a call under its policy: the bulkhead's slot it holds, whether it
/// may be attempted again, and how many times it was.
pub(crate) struct Run<'p, I> {
    policy: &'p ResiliencePolicy<I>,
    _slot: Option<SemaphorePermit<'p>>,
    replayed: bool,
    seed: u64,
    resent: u32,
}

impl<I: Instance> ResiliencePolicy<I> {
    /// The policy `config` describes, refusing a combination of its settings
    /// that cannot work, named as the instance's variables.
    pub(crate) fn new(config: &ResilienceConfig) -> Result<Self, ConfigError> {
        let namespace = instance_namespace::<I>("resilience");
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
        let retries = config
            .retries
            .map_or(0, |retries| u32::try_from(retries).unwrap_or(u32::MAX));
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
                    config.timeout,
                )
            }),
            bulkhead: config
                .concurrency
                .map(|slots| Bulkhead::new(slots, config.concurrency_wait)),
            runs: AtomicU64::new(0),
            _instance: PhantomData,
        })
    }

    /// Run `call` once, under the policy's timeout, breaker and bulkhead, and
    /// never again: a call this runs may change what it calls, so a retry could
    /// do it twice.
    ///
    /// `call` is handed the attempt's [`CancellationToken`], cancelled when the
    /// attempt is given up.
    pub async fn run<T, E, F, Fut>(&self, call: F) -> Result<T, ResilienceError<E>>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let mut run = self.begin(false).await?;
        settled(run.attempt(call, judged).await)
    }

    /// Run the idempotent `call`, attempting it again after a failed or timed
    /// out attempt, up to the policy's retries, each retry waiting the
    /// policy's backoff first; an open breaker ends the run at once.
    ///
    /// Idempotent means one call or several leave the dependency as one does:
    /// a read, a `PUT`, a request carrying an idempotency key the dependency
    /// honours. `call` is handed each attempt's [`CancellationToken`],
    /// cancelled when that attempt is given up.
    pub async fn run_idempotent<T, E, F, Fut>(&self, mut call: F) -> Result<T, ResilienceError<E>>
    where
        F: FnMut(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let mut run = self.begin(true).await?;
        loop {
            let attempted = run.attempt(&mut call, judged).await;
            if !run.again(&attempted).await {
                return settled(attempted);
            }
        }
    }

    /// Open one run: refused at once while the breaker is open, then by a full
    /// bulkhead.
    pub(crate) async fn begin(&self, replayed: bool) -> Result<Run<'_, I>, Refusal> {
        if let Some(retry_after) = self.breaker.as_ref().and_then(Breaker::refusing) {
            return Err(Refusal::Open(retry_after));
        }
        let slot = match &self.bulkhead {
            Some(bulkhead) => match bulkhead.enter().await {
                Some(slot) => Some(slot),
                None => {
                    tracing::debug!(
                        target: TARGET,
                        instance = I::NAME,
                        "bulkhead full; the call is refused",
                    );
                    return Err(Refusal::Full);
                }
            },
            None => None,
        };
        let run = self.runs.fetch_add(1, Ordering::Relaxed);
        Ok(Run {
            policy: self,
            _slot: slot,
            replayed,
            seed: process_seed() ^ Backoff::seed(&run.to_le_bytes()),
            resent: 0,
        })
    }

    /// How long to wait before attempt `resent + 2`, after one that failed
    /// asking `asked`, or `None` when the run ends there.
    fn retry_wait(&self, resent: u32, asked: Option<Duration>, seed: u64) -> Option<Duration> {
        if resent >= self.retries {
            return None;
        }
        let failed = NonZeroU32::new(resent.saturating_add(1)).unwrap_or(NonZeroU32::MIN);
        let backoff = self.backoff.wait(failed, seed);
        match asked {
            Some(asked) if asked > self.backoff.ceiling() => {
                tracing::debug!(
                    target: TARGET,
                    instance = I::NAME,
                    asked_ms = millis(asked),
                    ceiling_ms = millis(self.backoff.ceiling()),
                    "the dependency asked a wait past the backoff's ceiling; not attempted again",
                );
                None
            }
            Some(asked) => Some(backoff.max(Backoff::honouring(asked, failed, seed))),
            None => Some(backoff),
        }
    }
}

impl<I: Instance> Run<'_, I> {
    /// How many times this run sent its call again so far.
    #[cfg(feature = "reqwest")]
    pub(crate) const fn resent(&self) -> u32 {
        self.resent
    }

    /// One attempt of `call`: admitted by the breaker, bounded by the timeout,
    /// its answer counted as `judge` reads it. A panic in the call counts as a
    /// failure and unwinds on, for the unit to contain as it would without the
    /// policy.
    pub(crate) async fn attempt<T, F, Fut>(
        &mut self,
        call: F,
        judge: impl FnOnce(&T) -> Verdict,
    ) -> Attempted<T>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = T>,
    {
        let pass = match self.policy.breaker.as_ref().map(Breaker::admit) {
            Some(Admission::Refused { retry_after }) => return Err(Refusal::Open(retry_after)),
            Some(Admission::Admitted(pass)) => Some(pass),
            None => None,
        };
        let cancel = CancellationToken::new();
        let given_up = cancel.clone().drop_guard();
        let answered = within(self.policy.timeout, async move { call(cancel).await }).await;
        match answered {
            Ok(answer) => {
                let _kept = given_up.disarm();
                let verdict = judge(&answer);
                if let Some(pass) = pass {
                    pass.settle(matches!(verdict, Verdict::Failed { .. }));
                }
                Ok((answer, verdict))
            }
            Err(Unanswered::Panicked(payload)) => {
                if let Some(pass) = pass {
                    pass.settle(true);
                }
                drop(given_up);
                std::panic::resume_unwind(payload)
            }
            Err(_elapsed) => {
                if let Some(pass) = pass {
                    pass.settle(true);
                }
                drop(given_up);
                Err(Refusal::TimedOut(self.policy.timeout))
            }
        }
    }

    /// Whether to attempt the call again after `attempted`, having waited the
    /// backoff when so: only a run that may be replayed, after a failure or a
    /// timeout, within the policy's retries.
    pub(crate) async fn again<T>(&mut self, attempted: &Attempted<T>) -> bool {
        if !self.replayed {
            return false;
        }
        let asked = match attempted {
            Ok((_, Verdict::Failed { asked })) => *asked,
            Err(Refusal::TimedOut(_)) => None,
            Ok((_, Verdict::Succeeded)) | Err(Refusal::Open(_) | Refusal::Full) => return false,
        };
        let Some(wait) = self.policy.retry_wait(self.resent, asked, self.seed) else {
            return false;
        };
        tracing::debug!(
            target: TARGET,
            instance = I::NAME,
            attempt = self.resent.saturating_add(1),
            wait_ms = millis(wait),
            "attempt failed; retrying the call",
        );
        tokio::time::sleep(wait).await;
        self.resent = self.resent.saturating_add(1);
        true
    }
}

impl<I: Instance> nest_rs_core::CarriesRemedy for ResiliencePolicy<I> {
    fn remedy() -> Cow<'static, str> {
        let import = match I::NAME {
            "" => Cow::Borrowed("nest_rs::resilience::ResilienceModule"),
            _ => Cow::Owned(format!(
                "nest_rs::resilience::ResilienceModule::<{}>",
                nest_rs_core::__private::short_type_name::<I>()
            )),
        };
        let variables = nest_rs_config::var_name(&instance_namespace::<I>("resilience"), "*");
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
            .finish_non_exhaustive()
    }
}

/// A call's answer counts as failed when it is an `Err`.
fn judged<T, E>(answer: &Result<T, E>) -> Verdict {
    match answer {
        Ok(_) => Verdict::Succeeded,
        Err(_) => Verdict::Failed { asked: None },
    }
}

/// A run's last attempt, told in the caller's terms.
fn settled<T, E>(attempted: Attempted<Result<T, E>>) -> Result<T, ResilienceError<E>> {
    match attempted {
        Ok((Ok(value), _)) => Ok(value),
        Ok((Err(error), _)) => Err(ResilienceError::Failed(error)),
        Err(refusal) => Err(refusal.into()),
    }
}

/// `wait` in whole milliseconds, as a line's `_ms` field says it.
fn millis(wait: Duration) -> u64 {
    u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU32;

    use nest_rs_core::{CarriesRemedy, instance};

    use super::*;

    #[instance]
    struct Upstream;

    #[derive(Debug, PartialEq, Eq)]
    struct Refused;

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

    #[tokio::test(start_paused = true)]
    async fn an_asked_wait_is_honoured_up_to_the_ceiling_and_past_it_ends_the_run() {
        let policy = retrying(3);
        let mut run = policy.begin(true).await.expect("a run");
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

    #[test]
    fn an_unmet_policy_names_its_instances_import() {
        assert_eq!(
            ResiliencePolicy::<Upstream>::remedy(),
            format!(
                "Import `nest_rs::resilience::ResilienceModule::<Upstream>`, which builds the \
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
}
