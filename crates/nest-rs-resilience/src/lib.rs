//! Resilience for nestrs: a named policy runs every call to a dependency an app
//! does not control — a payment provider, a carrier's API, a language model —
//! so a slow one holds no request past its timeout and a down one is not
//! hammered by every retry.
//!
//! Import [`ResilienceModule<I>`](ResilienceModule) in the module whose
//! provider injects `Arc<ResiliencePolicy<I>>`, for an instance marker
//! `#[instance] struct I` naming the dependency
//! (`<PREFIX>_RESILIENCE__<INSTANCE>__*`, [`ResilienceConfig`]), and decorate
//! the method making the call: [`#[resilient(field)]`](macro@resilient) runs
//! its body once, `#[resilient(field, idempotent)]` again after an `Err` or a
//! timeout, up to its retries. A call that is not a whole method runs through
//! [`ResiliencePolicy::run`] or [`ResiliencePolicy::run_idempotent`]. An
//! attempt makes a [`CancellationToken`] only for a call that reads it: a
//! decorated method names it in its body with the `cancel` key, and a closure
//! takes it through [`ResiliencePolicy::run_cancellable`] or
//! [`ResiliencePolicy::run_idempotent_cancellable`]. Every
//! attempt is bounded by the policy's timeout and the unit's deadline,
//! whichever ends first — and, made from an attempt of another policy, by
//! that one's — and counted in its circuit breaker; the bulkhead
//! bounds how many run at once and how many wait for a slot, and the rate
//! limit how many attempts are sent per window, a retry included. Every `Err`
//! a call answers is its dependency failing — counted in the breaker, and sent
//! again when the call is idempotent — unless its method says otherwise, key
//! by key: `retry_if` says which `Err` is sent again and `retry_ok_if` which
//! `Ok`, and `record_if` which `Err` is a failure in the breaker, one it
//! refuses counting there as neither a failure nor a success; and `fallback`
//! names the method answering in place of the error a run ends on, the call's
//! own or the policy's refusal, `fallback_if` saying which. A refusal — a
//! timeout, an open breaker, a full bulkhead, a reached rate limit — is a
//! [`ResilienceError`] every edge answers `503`, an open breaker's and a
//! reached rate limit's with the `Retry-After` each knows. A breaker's state is
//! read through [`ResiliencePolicy::breaker_state`], and switched by hand
//! through [`ResiliencePolicy::open_breaker`] and
//! [`ResiliencePolicy::close_breaker`]. Under the `reqwest` feature,
//! [`ResilienceMiddleware`] runs the app's own reqwest client under a policy.
//!
//! **A policy tells what it does three ways, each with facts of its own.** A
//! line on [`TARGET`], for an operator. At `debug`: a retry, once it is
//! certain; a retry given up, the dependency asking a wait past the backoff's
//! ceiling or the unit's deadline leaving it no time; an attempt's savepoint
//! the database could not settle; a wait for a token of the rate; a call the
//! bulkhead or the rate refuses; and a fallback answering. At `warn`: a
//! breaker's transition.
//! A call an open breaker refuses, a call its unit had no time left to send
//! and a run ending on a timeout file no line of their own: the breaker said
//! it opened once, and each caller is answered its refusal. An event in code: [`ResiliencePolicy::events`] opens a
//! subscription to each retry, attempt given up at its bound, call refused
//! unsent, transition and fallback as a [`ResilienceEvent`]; a wait, a retry
//! given up and an unsettled savepoint are lines alone. And what it holds
//! now, read through
//! [`breaker_state`](ResiliencePolicy::breaker_state),
//! [`breaker_stats`](ResiliencePolicy::breaker_stats),
//! [`concurrency_running`](ResiliencePolicy::concurrency_running),
//! [`concurrency_waiting`](ResiliencePolicy::concurrency_waiting) and
//! [`rate_available`](ResiliencePolicy::rate_available).
//!
//! Under the `opentelemetry` feature a policy counts on the global meter, by
//! its instance, each transition of its breaker, each call it sent again,
//! each run it ended on its own refusal and each fallback answering, and its
//! reads are observed there. The meter is the one `OpenTelemetry::init()`
//! seats: a counter first recorded before it counts there from its next
//! record, and a policy run before it reports its reads from its next run. A
//! count is
//! not always an event's: a retry's line and event are told once the retry is
//! certain — as it is decided, or once its attempt's savepoint rolled back —
//! and its counter counts the call once it is sent again, so a retry the
//! policy then refuses unsent — a breaker that opened, a rate with no token —
//! is told, and counted as the refusal its run ends on, never as a retry; and
//! a timeout of an attempt that left a retry to make is told and counts on no
//! counter, where the run's last attempt given up counts among the refusals.
//!
//! **A policy's state lives in its process.** Each replica keeps its own
//! breaker, bulkhead and rate, so a dependency failing for one replica is
//! refused by that one alone, a breaker opens on the failures its own replica
//! saw, and a rate limit counts the attempts its own replica sent. The state is
//! not swappable: no store shares it across replicas.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — every line a policy files: at `debug` its
/// retries and the ones it gives up, an attempt's savepoint the database
/// could not settle, its waits for a token of the rate, the calls its
/// bulkhead and its rate refuse and its fallbacks answering, and at `warn`
/// its breaker's transitions.
pub const TARGET: &str = "nest_rs::resilience";

mod attempting;
#[cfg(feature = "reqwest")]
mod body;
mod breaker;
mod bulkhead;
mod call;
mod config;
mod counters;
mod error;
mod events;
#[cfg(feature = "reqwest")]
mod held;
mod mechanisms;
#[cfg(feature = "reqwest")]
mod middleware;
mod millis;
mod module;
mod observed;
mod policy;
mod rate;
mod resilient;
mod run;
mod savepoint;
mod teller;
mod told;
mod verdict;

pub use breaker::{BreakerPhase, BreakerState, BreakerStats};
pub use config::ResilienceConfig;
pub use error::{BreakerError, ResilienceError, ResilienceRefusal};
pub use events::{ResilienceEvent, ResilienceEvents};
#[cfg(feature = "reqwest")]
pub use middleware::ResilienceMiddleware;
pub use module::{ResilienceModule, ResilienceSetup};
pub use policy::ResiliencePolicy;

/// Runs a method calling a dependency under the policy a field of `self` holds,
/// with no closure and no `run` to call: `#[resilient(field)]` sends the call
/// once, `#[resilient(field, idempotent)]` again after an `Err` or a timeout,
/// and `timeout` and `retries` pin this method's own. `retry_if`, `retry_ok_if`
/// and `record_if` say how an answer counts — which `Err` and which `Ok` is
/// sent again, which `Err` is a failure in the circuit breaker, any other
/// `Err` counting there as neither a failure nor a success; `fallback`
/// names the method answering in place of the error a run ends on, the call's
/// own or the policy's refusal, and `fallback_if` which errors it answers for;
/// `cancel` names the attempt's [`CancellationToken`] in the body. The method's
/// error is a [`std::error::Error`], run once or again: its chain is the
/// `error` of the line a retry files and of the `warn` its failure files when
/// it opens the breaker.
///
/// ```
/// use std::sync::Arc;
///
/// use nest_rs_core::{App, injectable, instance, module};
/// use nest_rs_resilience::{
///     CancellationToken, ResilienceError, ResilienceModule, ResiliencePolicy, resilient,
/// };
///
/// #[instance]
/// pub struct Courier;
///
/// #[derive(Debug, thiserror::Error)]
/// #[error("the courier did not answer")]
/// pub struct CourierDown;
///
/// /// The courier's own API, down here: its failure, never the caller's input,
/// /// is the `Err` the breaker counts.
/// async fn courier_api(_parcel: &str) -> Result<(), CourierDown> {
///     Err(CourierDown)
/// }
///
/// /// What the courier's scale answers when it weighs nothing.
/// #[derive(Debug, thiserror::Error)]
/// pub enum NoWeight {
///     #[error("the scale is busy")]
///     Busy,
///     #[error("the scale did not answer")]
///     Down,
///     #[error("the courier knows no such parcel")]
///     Unknown,
/// }
///
/// impl NoWeight {
///     fn is_busy(&self) -> bool {
///         matches!(self, Self::Busy)
///     }
/// }
///
/// /// The courier's scale, down here but for a parcel it does not know. It
/// /// stops the work it started once `stop` is cancelled.
/// async fn courier_scale(parcel: &str, stop: CancellationToken) -> Result<u32, NoWeight> {
///     match parcel {
///         "" => Err(NoWeight::Unknown),
///         _ if stop.is_cancelled() => Err(NoWeight::Busy),
///         _ => Err(NoWeight::Down),
///     }
/// }
///
/// #[injectable]
/// pub struct ShipmentsService {
///     #[inject]
///     courier: Arc<ResiliencePolicy<Courier>>,
/// }
///
/// impl ShipmentsService {
///     #[resilient(courier, idempotent, timeout = "2s")]
///     pub async fn track(&self, parcel: &str) -> Result<String, ResilienceError<CourierDown>> {
///         Ok(format!("{parcel} is on its way"))
///     }
///
///     #[resilient(courier)]
///     pub async fn ship(&self, parcel: String) -> Result<(), ResilienceError<CourierDown>> {
///         courier_api(&parcel).await
///     }
///
///     /// Asked again while the scale is busy; a parcel the courier does not
///     /// know is its answer, no failure of its; and while the policy refuses
///     /// the call or the scale fails, the weight declared answers.
///     #[resilient(
///         courier,
///         idempotent,
///         retry_if = NoWeight::is_busy,
///         record_if = |error| !matches!(error, NoWeight::Unknown),
///         fallback = as_declared,
///         fallback_if = |error| !matches!(error, ResilienceError::Failed(NoWeight::Unknown)),
///         cancel = token
///     )]
///     pub async fn weigh(
///         &self,
///         parcel: &str,
///         declared: u32,
///     ) -> Result<u32, ResilienceError<NoWeight>> {
///         courier_scale(parcel, token).await
///     }
///
///     /// The fallback: `weigh`'s arguments, in order, then the error its run
///     /// ended on, answering what `weigh` answers.
///     async fn as_declared(
///         &self,
///         parcel: &str,
///         declared: u32,
///         error: ResilienceError<NoWeight>,
///     ) -> Result<u32, ResilienceError<NoWeight>> {
///         Ok(declared)
///     }
/// }
///
/// #[module(imports = [ResilienceModule<Courier>], providers = [ShipmentsService])]
/// pub struct ShipmentsModule;
///
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// let app = App::builder().module::<ShipmentsModule>().build().await?;
/// let shipments = app.container().get::<ShipmentsService>().expect("provided");
/// assert_eq!(shipments.track("PARCEL-1").await?, "PARCEL-1 is on its way");
/// assert!(matches!(
///     shipments.ship("PARCEL-2".to_owned()).await,
///     Err(ResilienceError::Failed(CourierDown))
/// ));
/// assert_eq!(shipments.weigh("PARCEL-3", 8).await?, 8);
/// assert!(matches!(
///     shipments.weigh("", 8).await,
///     Err(ResilienceError::Failed(NoWeight::Unknown))
/// ));
/// # Ok(())
/// # }
/// ```
pub use nest_rs_resilience_macros::resilient;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::resilient::{
        ChainedError, Fallback, FallbackIf, Judge, PolicyField, RecordIf, ResilientAnswer,
        ResilientMethod, RetryIf, RetryOkIf, attempt, every, fall_back, judging, none, policy,
        resilient_method, run, run_cancellable, run_idempotent, run_idempotent_cancellable,
    };
}

/// What an attempt is handed when its call takes one — a closure run through
/// [`ResiliencePolicy::run_cancellable`] or
/// [`ResiliencePolicy::run_idempotent_cancellable`], and a decorated method
/// whose `cancel` names it: cancelled when the policy gives
/// the attempt up, so work the call handed to another task stops with it.
pub use tokio_util::sync::CancellationToken;
