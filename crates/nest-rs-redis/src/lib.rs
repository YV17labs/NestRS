//! Redis for nestrs — one crate, one connection, one binding per port.
//!
//! [`RedisModule::for_root`] opens the one multiplexed [`RedisConnection`]
//! (`<PREFIX>_REDIS__*`; a `rediss://` URL encrypts it and verifies Redis's
//! certificate, against what [`RedisTls`] trusts); the bindings sit beside it in
//! the composition root and share it:
//!
//! - **queue** — [`RedisQueueModule`] binds the queue port over it, on Redis
//!   Streams: the portable `dyn JobProducer` a feature injects to
//!   `.push(AudioQueue, job, None).await?`, and the consumer the port's
//!   `QueueWorker` runs every `#[process]` method through in an app that also
//!   imports `QueueModule`.
//! - **throttler** (feature) — [`RedisThrottlerModule`] binds the
//!   cross-process `dyn ThrottlerStore` the `nest-rs-throttler` guard injects.
//! - **schedule** (feature) — [`RedisScheduleModule`] binds the
//!   `dyn OccurrenceLock` a scheduled job declared `replicas = "one"` claims
//!   each occurrence through, so one replica of the deployment fires it.
//!
//! The queue contract lives in [`nest-rs-queue`](::nest_rs_queue) (the
//! [`Job`] marker, the [`ProcessMethod`] inventory, the [`JobProducer`] seam and
//! the capabilities a backend declares); this crate is Redis's binding of it,
//! written on the `redis` client directly. Swapping storage means writing a
//! different `nest-rs-<storage>` crate against the same abstractions; the
//! macro and application code stay unchanged.
//!
//! [`Job`]: ::nest_rs_queue::Job
//! [`ProcessMethod`]: ::nest_rs_queue::ProcessMethod
//! [`JobProducer`]: ::nest_rs_queue::JobProducer

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — the shared connection's own events (reaching
/// Redis at boot). Declared by the crate that owns the concern: the queue's
/// and the throttler's events stay on their ports' targets
/// (`nest_rs::queue`, `nest_rs::throttler`), because a target's one job is to
/// say where an event came from, and connecting to Redis is neither port's.
pub const TARGET: &str = "nest_rs::redis";

mod backend;
mod config;
mod connection;
mod error;
mod layout;
mod module;
mod queue;
#[cfg(feature = "schedule")]
mod schedule;
#[cfg(feature = "throttler")]
mod throttler;
mod tls;

pub use config::RedisConfig;
pub use connection::RedisConnection;
pub use error::RedisError;
pub use module::{RedisModule, RedisSetup};
pub use queue::{RedisQueueConfig, RedisQueueModule, RedisQueueProducer, RedisQueueSetup};
#[cfg(feature = "schedule")]
pub use schedule::{RedisOccurrenceLock, RedisScheduleModule};
#[cfg(feature = "throttler")]
pub use throttler::{RedisThrottler, RedisThrottlerModule};
pub use tls::{RedisTls, RedisTlsIdentity};
