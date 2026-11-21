//! What `#[resilient]` expands to: the decorated method a run comes from, the
//! probes reading the field it names and what it answers, the two runs its body
//! goes through, and the stand-in a refused method's body is checked against.

use std::error::Error;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_core::Instance;
use tokio_util::sync::CancellationToken;

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
    }
}

impl ResilientMethod {
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
}

/// The error an idempotent `#[resilient]` method answers: one whose chain a
/// retry reads.
#[diagnostic::on_unimplemented(
    message = "an idempotent `#[resilient]` method's error is a `std::error::Error`, and `{Self}` \
               is not one",
    label = "what the method answers",
    note = "before a retry, the policy reads the error's chain for the `Retry-After` a `Problem` \
            in it asks"
)]
pub trait ChainedError: Error + 'static {}

#[diagnostic::do_not_recommend]
impl<E: Error + 'static> ChainedError for E {}

/// `call` run once under `policy`, as `method`'s body.
pub async fn run<R, I, C, F>(policy: &ResiliencePolicy<I>, method: ResilientMethod, call: C) -> R
where
    R: ResilientAnswer,
    I: Instance,
    C: FnOnce(CancellationToken) -> F,
    F: Future<Output = R::Attempt>,
{
    let ran = policy
        .run_for(Some(method), |cancel| {
            let attempt = call(cancel);
            async move { R::split(attempt.await) }
        })
        .await;
    R::answered(ran)
}

/// `call` run under `policy` as `method`'s idempotent body, attempted again
/// after a failure or a timeout.
pub async fn run_idempotent<R, I, C, F>(
    policy: &ResiliencePolicy<I>,
    method: ResilientMethod,
    mut call: C,
) -> R
where
    R: ResilientAnswer,
    R::Err: ChainedError,
    I: Instance,
    C: FnMut(CancellationToken) -> F,
    F: Future<Output = R::Attempt>,
{
    let ran = policy
        .run_idempotent_for(Some(method), |cancel| {
            let attempt = call(cancel);
            async move { R::split(attempt.await) }
        })
        .await;
    R::answered(ran)
}

/// What the body of a method `#[resilient]` refused is checked as: one attempt
/// that nothing runs, so the method reports its refusal and no error of its
/// body's, written as if it answered `Result<T, E>`.
pub fn refused<R, C, F>(call: C) -> R
where
    R: ResilientAnswer,
    C: FnOnce(CancellationToken) -> F,
    F: Future<Output = R::Attempt>,
{
    drop(call);
    unreachable!("a method `#[resilient]` refused does not compile")
}
