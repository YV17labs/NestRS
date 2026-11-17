//! Redis for nestrs — one crate, one connection per instance, one binding per
//! port.
//!
//! [`RedisModule`] opens the default instance's [`RedisConnection`]
//! (`<PREFIX>_REDIS__*`) over the [`RedisTopology`] the URL's scheme declares —
//! one server (`rediss://`), the primary Sentinel names (`rediss-sentinel://`),
//! or a Cluster (`rediss-cluster://`), every connection encrypted and verified
//! against what [`RedisConfig::tls`] trusts. That is the crate without a feature. Each
//! binding is a feature of its own, named for the port it binds, which imports
//! the connection and shares it on every topology, so the composition root names
//! a binding alone, and [`RedisModule`] only to pin it:
//!
//! - **queue** — `RedisQueueModule` binds the queue port over it, on Redis
//!   Streams: the port's `Jobs` a feature injects to
//!   `.push(AudioQueue, job).await?`, and the consumer the port's
//!   `QueueWorker` runs every `#[process]` method through in an app reaching
//!   one, whose host brings `QueueModule`.
//! - **throttler** — `RedisThrottlerModule` binds the cross-process
//!   `dyn ThrottlerStore` the `nest-rs-throttler` guard injects.
//! - **schedule** — `RedisScheduleModule` binds the `dyn OccurrenceLock` a
//!   scheduled job declared `replicas = "one"` claims each occurrence through,
//!   so one replica of the deployment fires it.
//!
//! A binding pulls its port's crate, which an app using Redis for another port
//! does not compile. Under the `health` feature, each instance
//! [`RedisModule`] opens reports its own readiness: a `PING`, which
//! an app importing `HealthModule` runs on the readiness and startup probes as
//! `redis`, or `redis:cache` for a named instance — no import of its own.
//!
//! An app opening a second Valkey — the queue's refuses `allkeys-*` eviction, a
//! cache's lives by it — names it with an instance marker:
//! the typed import `RedisModule::<Cache>` reads `<PREFIX>_REDIS__CACHE__*` and
//! provides `RedisConnection<Cache>`, and a binding over a named instance is a
//! typed import too, `RedisScheduleModule::<Locks>`. Every refusal names the
//! failing instance's variables, and every line the connection files carries
//! it as `instance`.
//!
//! The connection attaches no close to the app's way down: a Valkey or Redis
//! connection owes its server no goodbye — `QUIT` is deprecated since Redis
//! 7.2 in favour of closing the socket — so it ends when the app's container
//! drops, after every shutdown hook.

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
mod commands;
mod config;
mod connection;
mod error;
#[cfg(feature = "health")]
mod health;
mod instance;
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
pub use topology::RedisTopology;
