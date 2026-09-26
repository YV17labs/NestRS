//! Typed errors for the Redis substrate.
//!
//! Framework crates surface `thiserror` enums, not `anyhow`. Opening the shared
//! connection is a Redis-specific step, so it carries its own error here; the
//! producer surface (the `JobProducer` impl on
//! [`RedisQueueProducer`](crate::RedisQueueProducer)) instead speaks the
//! backend-agnostic
//! [`QueueError`](::nest_rs_queue::QueueError), wrapping a Redis push failure as
//! its opaque `Backend` source.

use thiserror::Error;

/// A failure opening the shared [`RedisConnection`](crate::RedisConnection)
/// from its configuration.
///
/// Concern-prefixed (`RedisError`, not a generic `ConnectionError`) to match
/// the house pattern — `ConfigError`, `StorageError`, `QueueError` — and avoid
/// a name collision when an app imports several infra errors at once.
///
/// Every variant names the endpoint as the address the client dials — never the
/// URL, which may embed a password, and every one of these strings reaches logs
/// and stderr.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RedisError {
    /// The URL could not configure a connection — malformed, or a scheme the
    /// client does not speak. It fails the same way on every attempt, so it is
    /// refused at once rather than retried for the whole budget.
    #[error(
        "invalid Redis URL {endpoint}: check {url_var}",
        url_var = ::nest_rs_config::spellings("redis", "URL"),
    )]
    InvalidUrl {
        /// The address the client dials, never the URL.
        endpoint: String,
        /// Why the client rejected it.
        #[source]
        source: redis::RedisError,
    },

    /// Redis answered, and refused in a way every attempt would repeat —
    /// credentials, an ACL denying the proof, a database index out of range.
    /// That is not an outage, and retrying it would only tell the operator to
    /// look at the network — so it fails at once, naming the variable that
    /// holds the URL, with Redis's answer as the source.
    #[error(
        "Redis at {endpoint} refused the connection: check {url_var}",
        url_var = ::nest_rs_config::spellings("redis", "URL"),
    )]
    Refused {
        /// The address the client dials, never the URL.
        endpoint: String,
        /// What Redis answered.
        #[source]
        source: redis::RedisError,
    },

    /// The connect budget elapsed with the backend still unreachable. Carries
    /// the redacted endpoint and the knob to widen, because this is the boot
    /// error an operator reads at 3am — an unreachable queue used to be
    /// indistinguishable from a hung process.
    #[error(
        "could not reach Redis at {endpoint} within {budget:?} ({attempts} attempt(s)): \
         check {url_var}, or widen the budget with {timeout_var}",
        url_var = ::nest_rs_config::spellings("redis", "URL"),
        timeout_var = ::nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS"),
    )]
    Unreachable {
        /// The address the client dials, never the URL.
        endpoint: String,
        /// The budget that elapsed.
        budget: std::time::Duration,
        /// How many connect attempts were made inside it.
        attempts: u32,
        /// The last transport failure, when the budget ran out after an
        /// outright error rather than mid-attempt.
        #[source]
        source: Option<redis::RedisError>,
    },
}
