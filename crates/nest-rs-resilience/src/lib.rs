//! Resilience for nestrs: what protects an **outbound call to a failing
//! dependency** — a payment provider, a carrier API, a language model.
//!
//! A [`Policy`] is named for the dependency it guards and shared by every
//! caller of it (it is cheap to clone: one breaker, one bulkhead, one set of
//! counters behind an `Arc`). It composes, outermost first:
//!
//! 1. a **bulkhead** ([`Bulkhead`]) bounding how many calls run at once, and
//!    for how long a call may queue for a slot — `BULKHEAD_FULL`, `503`;
//! 2. a **circuit breaker** ([`CircuitBreaker`]) over a count window, refusing
//!    every call while it is open and letting probes through once half-open —
//!    `CIRCUIT_OPEN`, `503` with `Retry-After`;
//! 3. **retries** ([`Retry`]) with the framework's jittered exponential
//!    [`Backoff`](nest_rs_core::Backoff), applied **only** to a call its caller
//!    declares idempotent ([`Policy::run_idempotent`], [`Call::idempotent`]);
//! 4. a **per-attempt timeout**, which cancels the attempt's
//!    [`CancellationToken`] and drops its
//!    future — `TIMEOUT`, `504`;
//! 5. an overall **deadline** bounding every attempt and every wait between
//!    them, so no call through a policy waits unboundedly.
//!
//! ```
//! use std::time::Duration;
//! use nest_rs_resilience::{CircuitBreaker, Policy, ResilienceError, Retry};
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let carrier = Policy::builder("carrier")
//!     .timeout(Duration::from_secs(2))
//!     .retry(Retry::DEFAULT)
//!     .circuit_breaker(CircuitBreaker::DEFAULT)
//!     .build()?;
//!
//! let rate: Result<u32, ResilienceError<std::io::Error>> = carrier
//!     .run_idempotent(|_cancel| async { Ok(42) })
//!     .await;
//! assert_eq!(rate?, 42);
//! # Ok(())
//! # }
//! ```
//!
//! [`ResilienceError`] implements [`ToProblem`](nest_rs_core::ToProblem)
//! whenever the call's own error does, so `?` in a handler answers the
//! policy's refusals in each edge's standard form and the call's own error as
//! that error says.
//!
//! **State lives per process.** Each replica keeps its own breaker and
//! bulkhead: a dependency failing for every replica opens every replica's
//! breaker within its own window. A transition is logged at `warn` on
//! [`TARGET`], and counted in [`Policy::stats`].

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — breaker transitions, retries and refusals.
pub const TARGET: &str = "nest_rs::resilience";

mod breaker;
mod bulkhead;
mod call;
pub mod code;
mod config;
mod error;
mod policy;
pub mod presets;
mod registry;
mod retry;
mod stats;

pub use breaker::{CircuitBreaker, CircuitState};
pub use bulkhead::Bulkhead;
pub use call::{Call, Classify, EveryError};
pub use config::{BulkheadConfig, CircuitBreakerConfig, InvalidPolicy, PolicyConfig, RetryConfig};
pub use error::ResilienceError;
pub use policy::{Policy, PolicyBuilder};
pub use registry::{DuplicatePolicy, Policies, UnknownPolicy};
pub use retry::Retry;
pub use stats::PolicyStats;
pub use tokio_util::sync::CancellationToken;
