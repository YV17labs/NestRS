//! Redis for nestrs — one crate, one connection, one binding per port.
//!
//! [`RedisModule::for_root`] opens the one multiplexed [`RedisConnection`]
//! (`NESTRS_REDIS__*`); the bindings sit beside it in the composition root and
//! share it:
//!
//! - **queue** — [`RedisQueueModule`] binds the portable `dyn JobProducer`
//!   over it: inject `Arc<dyn JobProducer>` and call
//!   `.push_to::<Q>(job).await?`.
//! - **worker** — [`RedisWorkerModule`] attaches the [`RedisWorker`] transport,
//!   which drains the `ProcessMethod` inventory the `#[processor]` macro feeds
//!   and runs one apalis worker per method. Producer-only apps skip it.
//! - **throttler** (feature) — [`RedisThrottlerModule`] binds the
//!   cross-process `dyn ThrottlerStore` the `nest-rs-throttler` guard injects.
//!
//! The queue contract lives in [`nest-rs-queue`](::nest_rs_queue) (the
//! [`Job`] marker, the [`Processor`] trait, the [`ProcessMethod`] inventory);
//! this crate is Redis's binding of it, built on apalis-redis. The user-facing
//! storage is **Redis**; apalis is an implementation detail this crate hides,
//! which is why the crate, its namespace and its span target all carry the
//! storage's word. Swapping storage means writing a different
//! `nest-rs-<storage>` crate against the same abstractions; the macro and
//! application code stay unchanged.
//!
//! [`Job`]: ::nest_rs_queue::Job
//! [`Processor`]: ::nest_rs_queue::Processor
//! [`ProcessMethod`]: ::nest_rs_queue::ProcessMethod

#![warn(missing_docs)]

/// This crate's span target — the shared connection's own events (reaching
/// Redis at boot). Declared by the crate that owns the concern: the queue's
/// and the throttler's events stay on their ports' targets
/// (`nest_rs::queue`, `nest_rs::throttler`), because a target's one job is to
/// say where an event came from, and connecting to Redis is neither port's.
pub const TARGET: &str = "nest_rs::redis";

mod config;
mod connection;
mod error;
mod module;
mod queue;
#[cfg(feature = "throttler")]
mod throttler;
mod worker;

pub use config::RedisConfig;
pub use connection::RedisConnection;
pub use error::RedisError;
pub use module::{RedisModule, RedisSetup};
pub use queue::{RedisQueueModule, RedisQueueProducer};
#[cfg(feature = "throttler")]
pub use throttler::{RedisThrottler, RedisThrottlerModule};
pub use worker::{RedisWorker, RedisWorkerConfig, RedisWorkerModule, RedisWorkerSetup};
