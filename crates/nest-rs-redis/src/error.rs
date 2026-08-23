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
/// from the configured URL.
///
/// Concern-prefixed (`RedisError`, not a generic `ConnectionError`) to match
/// the house pattern — `ConfigError`, `StorageError`, `QueueError` — and avoid
/// a name collision when an app imports several infra errors at once.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RedisError {
    /// The Redis connection could not be established.
    #[error("failed to connect to Redis")]
    Connect(#[from] redis::RedisError),

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
        /// The configured endpoint with any userinfo stripped — the URL may
        /// embed a password and this string reaches logs and stderr.
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
