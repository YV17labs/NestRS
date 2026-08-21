//! Producer side: the shared [`RedisQueueConnection`] and the [`RedisQueueModule`] seam
//! that builds it, plus the [`RedisQueueConfig`] both read.
//!
//! One folder per capability, like [`throttler`](crate::throttler) and
//! [`worker`](crate::worker) beside it. The queue is this crate's first use of
//! Redis and not its only one, so it earns a folder rather than the crate root
//! — a `src/module.rs` would be a module whose own path never says what it is a
//! module *of*, and the next capability behind a feature flag would have the
//! same problem with nowhere left to go.

mod config;
mod connection;
mod module;

pub use config::RedisQueueConfig;
pub use connection::{RedisQueue, RedisQueueConnection};
pub use module::{RedisQueueModule, RedisQueueSetup};
