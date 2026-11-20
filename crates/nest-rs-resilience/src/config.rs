//! [`PolicyConfig`] — a policy as configuration spells it, and
//! [`InvalidPolicy`], what refuses one.
//!
//! Durations are whole milliseconds, so a policy reads the same from a TOML
//! file, a JSON document or an environment variable.

use std::borrow::Cow;
use std::num::NonZeroU32;
use std::time::Duration;

use nest_rs_core::Backoff;
use serde::Deserialize;

use crate::breaker::CircuitBreaker;
use crate::bulkhead::Bulkhead;
use crate::policy::{Policy, PolicyBuilder};
use crate::retry::Retry;

/// A policy that could never let a call answer, or one configuration spells
/// wrong.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("resilience policy `{policy}` is refused: {reason}")]
pub struct InvalidPolicy {
    policy: String,
    reason: Cow<'static, str>,
}

impl InvalidPolicy {
    pub(crate) fn new(policy: impl Into<String>, reason: impl Into<Cow<'static, str>>) -> Self {
        Self {
            policy: policy.into(),
            reason: reason.into(),
        }
    }

    /// The name of the refused policy.
    pub fn policy(&self) -> &str {
        &self.policy
    }

    /// Why it is refused.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// A policy as configuration spells it; every part optional, every unknown
/// key refused.
///
/// ```
/// use nest_rs_resilience::PolicyConfig;
///
/// let config: PolicyConfig = serde_json::from_str(r#"{
///     "timeout_ms": 2000,
///     "retry": { "attempts": 3, "first_ms": 200, "ceiling_ms": 2000 },
///     "circuit_breaker": { "window": 20, "failure_rate_percent": 50, "open_for_ms": 30000 },
///     "bulkhead": { "max_concurrent": 32 }
/// }"#)?;
/// let policy = config.build("carrier")?;
/// assert_eq!(policy.available_slots(), Some(32));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct PolicyConfig {
    /// Each attempt's timeout, in milliseconds.
    pub timeout_ms: Option<u64>,
    /// The whole call's deadline, in milliseconds.
    pub deadline_ms: Option<u64>,
    /// Retries for an idempotent call.
    pub retry: Option<RetryConfig>,
    /// The circuit breaker.
    pub circuit_breaker: Option<CircuitBreakerConfig>,
    /// The bulkhead.
    pub bulkhead: Option<BulkheadConfig>,
}

/// [`Retry`] as configuration spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RetryConfig {
    /// Attempts in all, the first included; at least one.
    pub attempts: u32,
    /// About how long the first retry waits, in milliseconds.
    #[serde(default = "RetryConfig::default_first_ms")]
    pub first_ms: u64,
    /// The longest a wait grows to, in milliseconds.
    #[serde(default = "RetryConfig::default_ceiling_ms")]
    pub ceiling_ms: u64,
}

impl RetryConfig {
    const fn default_first_ms() -> u64 {
        200
    }

    const fn default_ceiling_ms() -> u64 {
        2_000
    }
}

/// [`CircuitBreaker`] as configuration spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct CircuitBreakerConfig {
    /// How many of the last attempts are judged; at least one.
    pub window: u32,
    /// The share of failures that opens the breaker, `1..=100`.
    pub failure_rate_percent: u8,
    /// How many attempts are recorded before the window is judged; the window
    /// by default.
    #[serde(default)]
    pub minimum_calls: Option<u32>,
    /// How long the breaker stays open, in milliseconds.
    pub open_for_ms: u64,
    /// How many half-open probes must succeed to close it; one by default.
    #[serde(default)]
    pub half_open_probes: Option<u32>,
}

/// [`Bulkhead`] as configuration spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BulkheadConfig {
    /// How many calls run at once; at least one.
    pub max_concurrent: u32,
    /// How long a call may queue for a slot, in milliseconds; none by default.
    #[serde(default)]
    pub max_wait_ms: u64,
}

fn positive(policy: &str, value: u32, what: &'static str) -> Result<NonZeroU32, InvalidPolicy> {
    NonZeroU32::new(value).ok_or_else(|| InvalidPolicy::new(policy, what))
}

impl PolicyConfig {
    /// The builder this configuration describes, for code to adjust further.
    ///
    /// # Errors
    ///
    /// [`InvalidPolicy`] when a count is zero or a rate is outside `1..=100`.
    pub fn builder(&self, name: impl Into<String>) -> Result<PolicyBuilder, InvalidPolicy> {
        let name = name.into();
        let mut builder = Policy::builder(name.clone());
        if let Some(ms) = self.timeout_ms {
            builder = builder.timeout(Duration::from_millis(ms));
        }
        if let Some(ms) = self.deadline_ms {
            builder = builder.deadline(Duration::from_millis(ms));
        }
        if let Some(retry) = self.retry {
            if retry.first_ms > retry.ceiling_ms {
                return Err(InvalidPolicy::new(
                    name,
                    "the retry's `first_ms` is at most its `ceiling_ms`",
                ));
            }
            builder = builder.retry(Retry::new(
                positive(
                    &name,
                    retry.attempts,
                    "the retry makes at least one attempt",
                )?,
                Backoff::new(
                    Duration::from_millis(retry.first_ms),
                    Duration::from_millis(retry.ceiling_ms),
                ),
            ));
        }
        if let Some(breaker) = self.circuit_breaker {
            if !(1..=100).contains(&breaker.failure_rate_percent) {
                return Err(InvalidPolicy::new(
                    name,
                    "the breaker's `failure_rate_percent` is within 1..=100",
                ));
            }
            let window = positive(
                &name,
                breaker.window,
                "the breaker's window is at least one",
            )?;
            let mut spec = CircuitBreaker::new(
                window,
                breaker.failure_rate_percent,
                Duration::from_millis(breaker.open_for_ms),
            );
            if let Some(minimum) = breaker.minimum_calls {
                if minimum == 0 || minimum > window.get() {
                    return Err(InvalidPolicy::new(
                        name,
                        "the breaker's `minimum_calls` is within 1..=window",
                    ));
                }
                spec = spec.with_minimum_calls(minimum);
            }
            if let Some(probes) = breaker.half_open_probes {
                spec = spec.with_half_open_probes(positive(
                    &name,
                    probes,
                    "the breaker lets at least one half-open probe through",
                )?);
            }
            builder = builder.circuit_breaker(spec);
        }
        if let Some(bulkhead) = self.bulkhead {
            builder = builder.bulkhead(
                Bulkhead::new(positive(
                    &name,
                    bulkhead.max_concurrent,
                    "the bulkhead lets at least one call through",
                )?)
                .with_max_wait(Duration::from_millis(bulkhead.max_wait_ms)),
            );
        }
        Ok(builder)
    }

    /// The policy this configuration describes.
    ///
    /// # Errors
    ///
    /// [`InvalidPolicy`], as [`builder`](Self::builder) and
    /// [`PolicyBuilder::build`] refuse.
    pub fn build(&self, name: impl Into<String>) -> Result<Policy, InvalidPolicy> {
        self.builder(name)?.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(json: &str) -> String {
        let config: PolicyConfig = serde_json::from_str(json).unwrap();
        config.build("p").unwrap_err().reason().to_owned()
    }

    #[test]
    fn an_empty_config_is_a_bare_policy() {
        let policy = PolicyConfig::default().build("bare").unwrap();
        assert_eq!(policy.available_slots(), None);
    }

    #[test]
    fn unknown_keys_are_refused() {
        assert!(serde_json::from_str::<PolicyConfig>(r#"{"timeout": 5}"#).is_err());
    }

    #[test]
    fn counts_and_rates_are_held_to_their_ranges() {
        assert_eq!(
            refused(r#"{"retry": {"attempts": 0}}"#),
            "the retry makes at least one attempt"
        );
        assert_eq!(
            refused(r#"{"retry": {"attempts": 2, "first_ms": 5000, "ceiling_ms": 10}}"#),
            "the retry's `first_ms` is at most its `ceiling_ms`"
        );
        assert_eq!(
            refused(
                r#"{"circuit_breaker": {"window": 5, "failure_rate_percent": 0, "open_for_ms": 1}}"#
            ),
            "the breaker's `failure_rate_percent` is within 1..=100"
        );
        assert_eq!(
            refused(
                r#"{"circuit_breaker": {"window": 5, "failure_rate_percent": 50, "open_for_ms": 1, "minimum_calls": 9}}"#
            ),
            "the breaker's `minimum_calls` is within 1..=window"
        );
        assert_eq!(
            refused(r#"{"bulkhead": {"max_concurrent": 0}}"#),
            "the bulkhead lets at least one call through"
        );
        assert_eq!(refused(r#"{"timeout_ms": 0}"#), "the timeout is above zero");
    }

    #[test]
    fn a_bad_name_is_refused() {
        let error = PolicyConfig::default().build(" padded ").unwrap_err();
        assert_eq!(error.policy(), " padded ");
        assert!(
            error
                .to_string()
                .starts_with("resilience policy ` padded ` is refused")
        );
    }
}
