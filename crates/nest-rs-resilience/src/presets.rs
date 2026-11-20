//! Named starting points for the common kinds of dependency: each answers a
//! [`PolicyBuilder`] to adjust before building.
//!
//! ```
//! use std::time::Duration;
//! use nest_rs_resilience::presets;
//!
//! let llm = presets::outbound_http("llm").timeout(Duration::from_secs(60)).build()?;
//! assert_eq!(llm.name(), "llm");
//! # Ok::<(), nest_rs_resilience::InvalidPolicy>(())
//! ```

use std::num::NonZeroU32;
use std::time::Duration;

use nest_rs_core::Backoff;

use crate::breaker::CircuitBreaker;
use crate::policy::{Policy, PolicyBuilder};
use crate::retry::Retry;

/// A partner's HTTP API: 10 s per attempt, three attempts for an idempotent
/// call ([`Retry::DEFAULT`]), the [`CircuitBreaker::DEFAULT`] breaker, and 30 s
/// for the whole call.
pub fn outbound_http(name: impl Into<String>) -> PolicyBuilder {
    Policy::builder(name)
        .timeout(Duration::from_secs(10))
        .deadline(Duration::from_secs(30))
        .retry(Retry::DEFAULT)
        .circuit_breaker(CircuitBreaker::DEFAULT)
}

/// A dependency on a request's critical path, which must fail fast: 2 s per
/// attempt, two attempts 100 ms apart for an idempotent call, a breaker
/// opening at half of the last ten (five recorded) for 15 s, and 5 s for the
/// whole call.
pub fn latency_critical(name: impl Into<String>) -> PolicyBuilder {
    Policy::builder(name)
        .timeout(Duration::from_secs(2))
        .deadline(Duration::from_secs(5))
        .retry(Retry::new(
            NonZeroU32::MIN.saturating_add(1),
            Backoff::new(Duration::from_millis(100), Duration::from_millis(500)),
        ))
        .circuit_breaker(
            CircuitBreaker::new(
                NonZeroU32::MIN.saturating_add(9),
                50,
                Duration::from_secs(15),
            )
            .with_minimum_calls(5),
        )
}

/// Work off the request path — a job, an export: 60 s per attempt, five
/// attempts for an idempotent call from 1 s up to 30 s apart, and five minutes
/// for the whole call. No breaker: a job's own retries pace it.
pub fn background(name: impl Into<String>) -> PolicyBuilder {
    Policy::builder(name)
        .timeout(Duration::from_secs(60))
        .deadline(Duration::from_secs(5 * 60))
        .retry(Retry::new(
            NonZeroU32::MIN.saturating_add(4),
            Backoff::new(Duration::from_secs(1), Duration::from_secs(30)),
        ))
}
