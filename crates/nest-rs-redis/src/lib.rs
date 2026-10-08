//! Redis for nestrs — one crate, one connection, one binding per port.
//!
//! [`RedisModule::for_root`] opens the one [`RedisConnection`]
//! (`<PREFIX>_REDIS__*`) over the [`RedisTopology`] the URL's scheme declares —
//! one server (`rediss://`), the primary Sentinel names (`rediss-sentinel://`),
//! or a Cluster (`rediss-cluster://`), every connection encrypted and verified
//! against what [`RedisTls`] trusts; the bindings sit beside it in the
//! composition root and share it, on every topology:
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
//! written on the `redis` client directly.
//!
//! [`Job`]: ::nest_rs_queue::Job
//! [`ProcessMethod`]: ::nest_rs_queue::ProcessMethod
//! [`JobProducer`]: ::nest_rs_queue::JobProducer

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — the shared connection's own events; the queue's
/// and the throttler's stay on their ports' targets.
pub const TARGET: &str = "nest_rs::redis";

mod backend;
mod cluster;
mod config;
mod connection;
mod error;
mod layout;
mod module;
mod queue;
#[cfg(feature = "schedule")]
mod schedule;
mod script;
mod sentinel;
mod standalone;
#[cfg(test)]
mod testing;
#[cfg(feature = "throttler")]
mod throttler;
mod tls;
mod topology;
mod url;

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
pub use topology::RedisTopology;
