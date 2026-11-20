//! [`Call`] — one call through a policy, as its caller declares it.

use std::future::Future;
use std::num::NonZeroU32;
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::TARGET;
use crate::error::ResilienceError;
use crate::policy::Policy;
use crate::stats::Counters;

/// Which of a call's errors count as the dependency failing: retried, when the
/// call is idempotent, and recorded as a failure by the breaker. An error that
/// does not — a `404`, a refused validation — is the dependency answering: it
/// ends the call at once and counts as a success.
///
/// Any `Fn(&E) -> bool` is one, answering `true` for a failure.
pub trait Classify<E> {
    /// Whether `error` is the dependency failing.
    fn is_failure(&self, error: &E) -> bool;
}

/// Every error is the dependency failing: the default classification.
#[derive(Clone, Copy, Debug, Default)]
pub struct EveryError;

impl<E> Classify<E> for EveryError {
    fn is_failure(&self, _error: &E) -> bool {
        true
    }
}

impl<E, F: Fn(&E) -> bool> Classify<E> for F {
    fn is_failure(&self, error: &E) -> bool {
        self(error)
    }
}

/// One call through a [`Policy`], as its caller declares it: whether it is
/// idempotent (retried) and which of its errors are failures.
///
/// ```
/// use nest_rs_resilience::{Policy, Retry};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let policy = Policy::builder("catalog").retry(Retry::DEFAULT).build()?;
///
/// #[derive(Debug)]
/// enum CatalogError { NotFound, Unavailable }
///
/// let answer: Result<&str, _> = policy
///     .call()
///     .idempotent()
///     // A missing product is the catalog answering, not failing.
///     .failure_if(|error: &CatalogError| matches!(error, CatalogError::Unavailable))
///     .run(|_cancel| async { Err::<&str, _>(CatalogError::NotFound) })
///     .await;
/// assert!(matches!(answer.unwrap_err().into_inner(), Some(CatalogError::NotFound)));
/// assert_eq!(policy.stats().retried, 0);
/// # Ok(())
/// # }
/// ```
#[must_use = "a call does nothing until `run` is awaited"]
#[derive(Debug)]
pub struct Call<'p, C = EveryError> {
    policy: &'p Policy,
    idempotent: bool,
    classify: C,
}

impl<'p> Call<'p> {
    pub(crate) const fn new(policy: &'p Policy) -> Self {
        Self {
            policy,
            idempotent: false,
            classify: EveryError,
        }
    }
}

impl<'p, C> Call<'p, C> {
    /// Declares the call idempotent: safe to make again, so the policy's
    /// [`Retry`](crate::Retry) applies to it.
    pub const fn idempotent(mut self) -> Self {
        self.idempotent = true;
        self
    }

    /// Counts as the dependency failing only the errors `classify` says are.
    pub fn failure_if<K>(self, classify: K) -> Call<'p, K> {
        Call {
            policy: self.policy,
            idempotent: self.idempotent,
            classify,
        }
    }

    /// Runs `attempt` through the policy. Each attempt receives a
    /// [`CancellationToken`] the policy cancels when the attempt's time is up,
    /// just before it drops the attempt's future.
    ///
    /// # Errors
    ///
    /// [`ResilienceError`]: a refusal of the policy's, or the call's own error
    /// from its last attempt.
    pub async fn run<T, E, F, Fut>(self, attempt: F) -> Result<T, ResilienceError<E>>
    where
        F: FnMut(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
        C: Classify<E>,
    {
        execute(self.policy, self.idempotent, &self.classify, attempt).await
    }

    /// Runs `attempt` as [`run`](Self::run) does, and answers what `fallback`
    /// makes of any error — a cached value, a default, a degraded answer.
    /// [`ResilienceError::is_rejected`] tells the fallback whether the call
    /// was made at all.
    ///
    /// # Errors
    ///
    /// What `fallback` answers.
    pub async fn run_or_else<T, E, F, Fut, B, BFut>(
        self,
        attempt: F,
        fallback: B,
    ) -> Result<T, ResilienceError<E>>
    where
        F: FnMut(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
        C: Classify<E>,
        B: FnOnce(ResilienceError<E>) -> BFut,
        BFut: Future<Output = Result<T, ResilienceError<E>>>,
    {
        match self.run(attempt).await {
            Ok(value) => Ok(value),
            Err(error) => fallback(error).await,
        }
    }
}

/// The remaining time before `deadline`, if any.
fn remaining(deadline: Option<Instant>) -> Option<Duration> {
    deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()))
}

/// The shorter of two optional bounds.
fn tighter(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

async fn execute<T, E, F, Fut, C>(
    policy: &Policy,
    idempotent: bool,
    classify: &C,
    mut attempt: F,
) -> Result<T, ResilienceError<E>>
where
    F: FnMut(CancellationToken) -> Fut,
    Fut: Future<Output = Result<T, E>>,
    C: Classify<E>,
{
    let inner = policy.inner();
    let counters = &inner.counters;
    Counters::bump(&counters.calls);
    let deadline = inner.deadline.map(|bound| Instant::now() + bound);

    let _slot = match &inner.bulkhead {
        Some(slots) => {
            if let Some(slot) = slots.acquire(remaining(deadline)).await {
                Some(slot)
            } else {
                Counters::bump(&counters.bulkhead_rejected);
                tracing::debug!(target: TARGET, policy = %inner.name, "bulkhead full");
                return Err(ResilienceError::BulkheadFull {
                    policy: inner.name.clone(),
                });
            }
        }
        None => None,
    };

    let attempts = match (&inner.retry, idempotent) {
        (Some(retry), true) => retry.attempts().get(),
        _ => 1,
    };
    let mut number = NonZeroU32::MIN;
    loop {
        let ticket = match &inner.breaker {
            Some(breaker) => match breaker.admit() {
                Ok(ticket) => Some(ticket),
                Err(retry_after) => {
                    Counters::bump(&counters.short_circuited);
                    return Err(ResilienceError::CircuitOpen {
                        policy: inner.name.clone(),
                        retry_after,
                    });
                }
            },
            None => None,
        };

        let budget = tighter(inner.timeout, remaining(deadline));
        let cancel = CancellationToken::new();
        let answered = match budget {
            Some(budget) => {
                let answer = tokio::time::timeout(budget, attempt(cancel.clone())).await;
                if answer.is_err() {
                    cancel.cancel();
                }
                answer.ok()
            }
            None => Some(attempt(cancel.clone()).await),
        };

        let last = match answered {
            Some(Ok(value)) => {
                if let Some(ticket) = ticket {
                    ticket.record(false, counters);
                }
                Counters::bump(&counters.succeeded);
                return Ok(value);
            }
            Some(Err(error)) if !classify.is_failure(&error) => {
                if let Some(ticket) = ticket {
                    ticket.record(false, counters);
                }
                Counters::bump(&counters.succeeded);
                return Err(ResilienceError::Failed {
                    policy: inner.name.clone(),
                    attempts: number.get(),
                    error,
                });
            }
            Some(Err(error)) => {
                if let Some(ticket) = ticket {
                    ticket.record(true, counters);
                }
                ResilienceError::Failed {
                    policy: inner.name.clone(),
                    attempts: number.get(),
                    error,
                }
            }
            None => {
                if let Some(ticket) = ticket {
                    ticket.record(true, counters);
                }
                Counters::bump(&counters.timed_out);
                ResilienceError::Timeout {
                    policy: inner.name.clone(),
                    after: budget.unwrap_or_default(),
                    attempts: number.get(),
                }
            }
        };

        let wait = inner
            .retry
            .map(|retry| retry.backoff().wait(number, inner.seed))
            .unwrap_or_default();
        let out_of_time = remaining(deadline).is_some_and(|left| left <= wait);
        if number.get() >= attempts || out_of_time {
            if matches!(last, ResilienceError::Failed { .. }) {
                Counters::bump(&counters.failed);
            }
            return Err(last);
        }
        Counters::bump(&counters.retried);
        tracing::debug!(
            target: TARGET,
            policy = %inner.name,
            attempt = number.get(),
            wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
            "retrying after a failed attempt",
        );
        tokio::time::sleep(wait).await;
        number = number.saturating_add(1);
    }
}
