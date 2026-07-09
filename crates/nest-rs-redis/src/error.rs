//! Typed errors for the Redis queue backend.
//!
//! Framework crates surface `thiserror` enums, not `anyhow`. Opening the shared
//! connection is a Redis-specific step, so it carries its own error here; the
//! producer surface ([`Queue::push`](crate::Queue::push) and the `JobProducer`
//! impl) instead speaks the backend-agnostic
//! [`QueueError`](::nest_rs_queue::QueueError), wrapping a Redis push failure as
//! its opaque `Backend` source.

use thiserror::Error;

/// A failure opening the shared Redis
/// [`QueueConnection`](crate::QueueConnection) from the configured URL.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConnectionError {
    /// The Redis connection could not be established.
    #[error("failed to connect to Redis")]
    Connect(#[from] apalis_redis::RedisError),
}
