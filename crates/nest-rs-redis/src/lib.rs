//! Redis-backed queue integration for nestrs — the first-class backend
//! that plugs into the abstractions defined by
//! [`nest-rs-queue`](::nest_rs_queue) (the [`Job`] marker, the [`Processor`]
//! trait, the [`ProcessMethod`] inventory the `#[processor]` macro feeds).
//!
//! Built on apalis-redis: durable, distributed queues with retries. The
//! user-facing storage is **Redis**; apalis is an implementation detail
//! this crate hides. Queue names are stringly-typed (a known cost: the
//! consuming `#[processor]` and every producer must agree on the literal).
//!
//! Two sides:
//! - **Consumer**: `#[processor]` on a provider + one `#[process(queue =
//!   "...")]` per method. The [`QueueWorker`] transport drains the
//!   `ProcessMethod` inventory the macro feeds and runs one apalis worker
//!   per method.
//! - **Producer**: inject [`QueueConnection`] and call
//!   `.of::<Job>("name").push(job).await?`.
//!
//! Connection is async, seeded at the composition root as a factory — apalis
//! types never leak into apps. Swapping storage means writing a different
//! `nest-rs-<storage>` crate against the same `nest-rs-queue` abstractions;
//! the macro and application code stay unchanged.
//!
//! ## Future expansion
//!
//! When Redis grows a second nestrs use (cache, pub/sub, distributed locks),
//! this crate adds a matching Cargo feature flag rather than spawning a
//! sibling crate — Redis is one external dependency, this is its one
//! integration home. The first such feature is **`throttler`**: a
//! cross-process [`RedisThrottler`] rate-limit store backing the
//! `nest-rs-throttler` guard over the shared [`QueueConnection`].
//!
//! [`Job`]: ::nest_rs_queue::Job
//! [`Processor`]: ::nest_rs_queue::Processor
//! [`ProcessMethod`]: ::nest_rs_queue::ProcessMethod

mod config;
mod connection;
mod error;
mod module;
#[cfg(feature = "throttler")]
mod throttler;
mod worker;

pub use config::QueueConfig;
pub use connection::{Queue, QueueConnection};
pub use error::ConnectionError;
pub use module::{QueueModule, QueueSetup};
#[cfg(feature = "throttler")]
pub use throttler::{RedisThrottler, RedisThrottlerModule, RedisThrottlerSetup};
pub use worker::{QueueWorker, QueueWorkerModule};
