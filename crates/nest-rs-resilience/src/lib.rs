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
//! its body once, `#[resilient(field, idempotent)]` again after a failure, up
//! to its retries. A call that needs its attempt's [`CancellationToken`], or is
//! not a whole method, runs through [`ResiliencePolicy::run`] or
//! [`ResiliencePolicy::run_idempotent`]. Every attempt is bounded by the
//! policy's timeout and the unit's deadline, whichever ends first, and counted
//! in its circuit breaker; the bulkhead bounds how many run at once and how
//! many wait for a slot, and the rate limit how many attempts are sent per
//! window, a retry included. A refusal — a timeout, an
//! open breaker, a full bulkhead, a reached rate limit — is a
//! [`ResilienceError`] every edge answers `503`, an open breaker's and a
//! reached rate limit's with the `Retry-After` each knows. A breaker's state is
//! read through [`ResiliencePolicy::breaker_state`], and switched by hand
//! through [`ResiliencePolicy::open_breaker`] and
//! [`ResiliencePolicy::close_breaker`]. Under the `reqwest` feature,
//! [`ResilienceMiddleware`] runs the app's own reqwest client under a policy.
//!
//! **A policy's state lives in its process.** Each replica keeps its own
//! breaker, bulkhead and rate, so a dependency failing for one replica is
//! refused by that one alone, a breaker opens on the failures its own replica
//! saw, and a rate limit counts the attempts its own replica sent. The state is
//! not swappable: no store shares it across replicas.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — a breaker's transitions and a policy's retries.
pub const TARGET: &str = "nest_rs::resilience";

mod breaker;
mod bulkhead;
mod config;
mod counters;
mod error;
#[cfg(feature = "reqwest")]
mod middleware;
mod millis;
mod module;
mod policy;
mod rate;
mod resilient;

pub use breaker::BreakerState;
pub use config::ResilienceConfig;
pub use error::{BreakerError, ResilienceError};
#[cfg(feature = "reqwest")]
pub use middleware::ResilienceMiddleware;
pub use module::{ResilienceModule, ResilienceSetup};
pub use policy::ResiliencePolicy;

/// Runs a method calling a dependency under the policy a field of `self` holds,
/// with no closure and no `run` to call: `#[resilient(field)]` sends the call
/// once, `#[resilient(field, idempotent)]` again after a failure or a timeout,
/// and `timeout` and `retries` pin this method's own.
///
/// ```
/// use std::sync::Arc;
///
/// use nest_rs_core::{App, injectable, instance, module};
/// use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy, resilient};
///
/// #[instance]
/// pub struct Courier;
///
/// #[derive(Debug, thiserror::Error)]
/// #[error("the courier refused the parcel")]
/// pub struct CourierError;
///
/// #[injectable]
/// pub struct ShipmentsService {
///     #[inject]
///     courier: Arc<ResiliencePolicy<Courier>>,
/// }
///
/// impl ShipmentsService {
///     #[resilient(courier, idempotent, timeout = "2s")]
///     pub async fn track(&self, parcel: &str) -> Result<String, ResilienceError<CourierError>> {
///         Ok(format!("{parcel} is on its way"))
///     }
///
///     #[resilient(courier)]
///     pub async fn ship(&self, parcel: String) -> Result<(), ResilienceError<CourierError>> {
///         if parcel.is_empty() {
///             return Err(CourierError);
///         }
///         Ok(())
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
///     shipments.ship(String::new()).await,
///     Err(ResilienceError::Failed(CourierError))
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
        ChainedError, PolicyField, ResilientAnswer, ResilientMethod, attempt, policy, refused,
        resilient_method, run, run_idempotent,
    };
}

/// What each attempt is handed: cancelled when the policy gives the attempt up,
/// so work the call handed to another task stops with it.
pub use tokio_util::sync::CancellationToken;
