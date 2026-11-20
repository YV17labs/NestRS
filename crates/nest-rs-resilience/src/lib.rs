//! Resilience for nestrs: a named policy an app wraps around every call to a
//! dependency it does not control — a payment provider, a carrier's API, a
//! language model — so a slow one holds no request past its timeout and a down
//! one is not hammered by every retry.
//!
//! Import [`ResilienceModule::<I>`](ResilienceModule) in the module whose
//! provider injects `Arc<ResiliencePolicy<I>>`, for an instance marker
//! `#[instance] struct I` naming the dependency
//! (`<PREFIX>_RESILIENCE__<INSTANCE>__*`, [`ResilienceConfig`]), and run each
//! call through it: [`ResiliencePolicy::run`] once,
//! [`ResiliencePolicy::run_idempotent`] again after a failure, up to its
//! retries. Every attempt is bounded by the policy's timeout and counted in
//! its circuit breaker, and the bulkhead bounds how many run at once. A
//! refusal — a timeout, an open breaker, a full bulkhead — is a
//! [`ResilienceError`] every edge answers `503`, an open breaker's with the
//! `Retry-After` it knows. Under the `reqwest` feature,
//! [`ResilienceMiddleware`] runs the app's own reqwest client under a policy.
//!
//! **A policy's state lives in its process.** Each replica keeps its own
//! breaker and bulkhead, so a dependency failing for one replica is refused by
//! that one alone, and a breaker opens on the failures its own replica saw.
//! The state is not swappable: no store shares it across replicas.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — a breaker's transitions and a policy's retries.
pub const TARGET: &str = "nest_rs::resilience";

mod breaker;
mod bulkhead;
mod config;
mod error;
#[cfg(feature = "reqwest")]
mod middleware;
mod module;
mod policy;

pub use config::ResilienceConfig;
pub use error::ResilienceError;
#[cfg(feature = "reqwest")]
pub use middleware::ResilienceMiddleware;
pub use module::{ResilienceModule, ResilienceSetup};
pub use policy::ResiliencePolicy;

use nest_rs_core::__private::millis;
/// What each attempt is handed: cancelled when the policy gives the attempt up,
/// so work the call handed to another task stops with it.
pub use tokio_util::sync::CancellationToken;
