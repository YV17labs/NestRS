//! Typed errors for the Redis substrate.
//!
//! Framework crates surface `thiserror` enums, not `anyhow`. Opening the shared
//! connection pool is a Redis-specific step, so it carries its own error here;
//! the producer surface (the `JobProducer` impl on
//! [`RedisQueueProducer`](crate::RedisQueueProducer)) instead speaks the
//! backend-agnostic
//! [`QueueError`](::nest_rs_queue::QueueError), wrapping a Redis push failure as
//! its opaque `Backend` source. The two crate-private errors below are what a
//! queue binding hands its port or its runtime.

use thiserror::Error;

/// A failure opening the shared [`RedisConnection`](crate::RedisConnection)
/// from the configured URL.
///
/// Concern-prefixed (`RedisError`, not a generic `ConnectionError`) to match
/// the house pattern — `ConfigError`, `StorageError`, `QueueError` — and avoid
/// a name collision when an app imports several infra errors at once.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RedisError {
    /// The URL could not configure a connection — malformed, or a scheme the
    /// client does not speak. It fails the same way on every attempt, so it is
    /// refused at once rather than retried for the whole budget.
    #[error(
        "invalid Redis URL {endpoint}: check {url_var}",
        url_var = ::nest_rs_config::var_name("redis", "URL"),
    )]
    InvalidUrl {
        /// The address the client dials, never the URL — which may embed a
        /// password, and this string reaches logs and stderr.
        endpoint: String,
        /// Why the client rejected it.
        #[source]
        source: deadpool_redis::CreatePoolError,
    },

    /// Redis answered, and refused in a way every attempt would repeat —
    /// credentials, an ACL denying the proof, a database index out of range.
    /// That is not an outage, and retrying it would only tell the operator to
    /// look at the network — so it fails at once, naming the variable that
    /// holds the URL, with Redis's answer as the source.
    #[error(
        "Redis at {endpoint} refused the connection: check {url_var}",
        url_var = ::nest_rs_config::var_name("redis", "URL"),
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
        "could not reach Redis at {endpoint} within {}s ({attempts} attempt(s)): \
         check {url_var}, or widen the budget with {timeout_var}",
        budget.as_secs(),
        url_var = ::nest_rs_config::var_name("redis", "URL"),
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
        source: Option<deadpool_redis::PoolError>,
    },
}

/// A queue name the Redis queue bindings cannot file. oxana keeps a queue under
/// its namespace prefix unless the name already starts with that prefix, in
/// which case it takes the name verbatim — so a queue named `nestrs:queue:dead`
/// would write onto the dead list itself.
#[derive(Debug, Error)]
#[error(
    "queue `{queue}` starts with `{namespace}`, the prefix of the Redis queue bindings' own keys — name the queue something else"
)]
pub(crate) struct ReservedQueueName {
    pub(crate) queue: String,
    pub(crate) namespace: &'static str,
}

/// Why a job failed, as the job runtime records it on the dead list.
#[derive(Debug, Error)]
#[error(transparent)]
pub(crate) struct Undelivered(pub(crate) Box<dyn std::error::Error + Send + Sync>);
