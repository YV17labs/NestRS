//! Redis for nestrs — one crate, one connection, one binding per port.
//!
//! [`RedisModule::for_root`] opens the one [`RedisConnection`]
//! (`<PREFIX>_REDIS__*`) over the [`RedisTopology`] the URL's scheme declares —
//! one server (`rediss://`), the primary Sentinel names (`rediss-sentinel://`),
//! or a Cluster (`rediss-cluster://`), every connection encrypted and verified
//! against what [`RedisTls`] trusts. That is the crate without a feature. Each
//! binding is a feature of its own, named for the port it binds, and sits beside
//! the connection in the composition root, sharing it on every topology:
//!
//! - **queue** — [`RedisQueueModule`] binds the queue port over it, on Redis
//!   Streams: the portable `dyn JobProducer` a feature injects to
//!   `.push(AudioQueue, job, None).await?`, and the consumer the port's
//!   `QueueWorker` runs every `#[process]` method through in an app that also
//!   imports `QueueModule`.
//! - **throttler** — [`RedisThrottlerModule`] binds the cross-process
//!   `dyn ThrottlerStore` the `nest-rs-throttler` guard injects.
//! - **schedule** — [`RedisScheduleModule`] binds the `dyn OccurrenceLock` a
//!   scheduled job declared `replicas = "one"` claims each occurrence through,
//!   so one replica of the deployment fires it.
//!
//! A binding pulls its port's crate, which an app using Redis for another port
//! does not compile.

#![warn(missing_docs)]
#![cfg_attr(
    not(feature = "queue"),
    expect(
        dead_code,
        reason = "the connection carries what its bindings send through it, and the queue's \
                  dedicated connection for a blocking read is one: a build without a binding \
                  leaves its part unused"
    )
)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — the shared connection's own events; the queue's
/// and the throttler's stay on their ports' targets.
pub const TARGET: &str = "nest_rs::redis";

mod cluster;
mod config;
mod connection;
mod error;
mod millis;
mod module;
#[cfg(feature = "queue")]
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
#[cfg(feature = "queue")]
pub use queue::{RedisQueueConfig, RedisQueueModule, RedisQueueProducer, RedisQueueSetup};
#[cfg(feature = "schedule")]
pub use schedule::{RedisOccurrenceLock, RedisScheduleModule};
#[cfg(feature = "throttler")]
pub use throttler::{RedisThrottler, RedisThrottlerModule};
pub use tls::{RedisTls, RedisTlsIdentity};
pub use topology::RedisTopology;
