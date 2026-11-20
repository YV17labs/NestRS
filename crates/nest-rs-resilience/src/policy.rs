//! [`Policy`] — the named protection every caller of one dependency shares.

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::{Backoff, process_seed};
use tokio_util::sync::CancellationToken;

use crate::breaker::{Breaker, CircuitBreaker, CircuitState};
use crate::bulkhead::{Bulkhead, Slots};
use crate::call::Call;
use crate::config::InvalidPolicy;
use crate::error::ResilienceError;
use crate::retry::Retry;
use crate::stats::{Counters, PolicyStats};

/// The protection around calls to one dependency, named for it and shared by
/// every caller: clone it freely, the clones share one breaker, one bulkhead
/// and one set of counters.
///
/// Built with [`Policy::builder`]; see the [crate docs](crate) for the order
/// its parts apply in.
#[derive(Clone)]
pub struct Policy {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub(crate) name: Arc<str>,
    pub(crate) timeout: Option<Duration>,
    pub(crate) deadline: Option<Duration>,
    pub(crate) retry: Option<Retry>,
    pub(crate) breaker: Option<Breaker>,
    pub(crate) bulkhead: Option<Slots>,
    pub(crate) seed: u64,
    pub(crate) counters: Counters,
}

impl Policy {
    /// A builder for the policy named `name`: no timeout, no retry, no
    /// breaker, no bulkhead until the builder adds them.
    pub fn builder(name: impl Into<String>) -> PolicyBuilder {
        PolicyBuilder {
            name: name.into(),
            timeout: None,
            deadline: None,
            retry: None,
            breaker: None,
            bulkhead: None,
        }
    }

    pub(crate) fn inner(&self) -> &Inner {
        &self.inner
    }

    /// The name the policy was built with — the dependency it guards.
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Where the breaker stands; [`CircuitState::Closed`] for a policy with
    /// none.
    pub fn circuit_state(&self) -> CircuitState {
        self.inner
            .breaker
            .as_ref()
            .map_or(CircuitState::Closed, Breaker::state)
    }

    /// How many bulkhead slots are free right now; `None` for a policy with no
    /// bulkhead.
    pub fn available_slots(&self) -> Option<usize> {
        self.inner.bulkhead.as_ref().map(Slots::available)
    }

    /// The policy's counters since the process started.
    pub fn stats(&self) -> PolicyStats {
        self.inner.counters.snapshot()
    }

    /// A call to declare before running it: idempotent or not, and which of
    /// its errors are the dependency failing.
    pub const fn call(&self) -> Call<'_> {
        Call::new(self)
    }

    /// Runs `attempt` once through the policy — never retried, since the
    /// caller does not declare it idempotent.
    ///
    /// # Errors
    ///
    /// [`ResilienceError`]: a refusal of the policy's, or the call's own error.
    pub async fn run<T, E, F, Fut>(&self, attempt: F) -> Result<T, ResilienceError<E>>
    where
        F: FnMut(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        self.call().run(attempt).await
    }

    /// Runs `attempt` through the policy, retried on the policy's
    /// [`Retry`]: the caller declares it safe to make again.
    ///
    /// # Errors
    ///
    /// [`ResilienceError`]: a refusal of the policy's, or the call's own error
    /// from its last attempt.
    pub async fn run_idempotent<T, E, F, Fut>(&self, attempt: F) -> Result<T, ResilienceError<E>>
    where
        F: FnMut(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        self.call().idempotent().run(attempt).await
    }
}

impl fmt::Debug for Policy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = &self.inner;
        f.debug_struct("Policy")
            .field("name", &inner.name)
            .field("timeout", &inner.timeout)
            .field("deadline", &inner.deadline)
            .field("retry", &inner.retry)
            .field("circuit_state", &self.circuit_state())
            .field("available_slots", &self.available_slots())
            .finish_non_exhaustive()
    }
}

/// How a [`Policy`] is put together, from [`Policy::builder`].
#[derive(Clone, Debug)]
#[must_use = "a builder does nothing until `build` is called"]
pub struct PolicyBuilder {
    name: String,
    timeout: Option<Duration>,
    deadline: Option<Duration>,
    retry: Option<Retry>,
    breaker: Option<CircuitBreaker>,
    bulkhead: Option<Bulkhead>,
}

impl PolicyBuilder {
    /// Gives each attempt at most `timeout`.
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Gives the whole call — its slot in the bulkhead, every attempt and
    /// every wait between them — at most `deadline`.
    pub const fn deadline(mut self, deadline: Duration) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Retries an idempotent call on `retry`.
    pub const fn retry(mut self, retry: Retry) -> Self {
        self.retry = Some(retry);
        self
    }

    /// Guards the dependency with `breaker`.
    pub const fn circuit_breaker(mut self, breaker: CircuitBreaker) -> Self {
        self.breaker = Some(breaker);
        self
    }

    /// Bounds the calls running at once with `bulkhead`.
    pub const fn bulkhead(mut self, bulkhead: Bulkhead) -> Self {
        self.bulkhead = Some(bulkhead);
        self
    }

    /// The policy.
    ///
    /// # Errors
    ///
    /// [`InvalidPolicy`] when the name is empty or not trimmed, or a timeout
    /// or deadline is zero — a policy that could never let a call answer.
    pub fn build(self) -> Result<Policy, InvalidPolicy> {
        let name = self.name;
        if name.is_empty() || name.trim() != name {
            return Err(InvalidPolicy::new(
                name,
                "a policy's name is not empty and has no surrounding whitespace",
            ));
        }
        if self.timeout.is_some_and(|timeout| timeout.is_zero()) {
            return Err(InvalidPolicy::new(name, "the timeout is above zero"));
        }
        if self.deadline.is_some_and(|deadline| deadline.is_zero()) {
            return Err(InvalidPolicy::new(name, "the deadline is above zero"));
        }
        let name: Arc<str> = Arc::from(name);
        Ok(Policy {
            inner: Arc::new(Inner {
                seed: Backoff::seed(name.as_bytes()) ^ process_seed(),
                breaker: self
                    .breaker
                    .map(|spec| Breaker::new(spec, Arc::clone(&name))),
                bulkhead: self.bulkhead.map(Slots::new),
                timeout: self.timeout,
                deadline: self.deadline,
                retry: self.retry,
                counters: Counters::default(),
                name,
            }),
        })
    }
}
