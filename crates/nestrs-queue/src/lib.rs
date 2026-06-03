//! Redis-backed, distributed job queues with retries. Queue names are
//! stringly-typed (a known cost: the consuming `#[processor]` and every
//! producer must agree on the literal).
//!
//! Two sides:
//! - **Consumer**: `#[processor(queue = "...")]` on a struct + `impl Processor`.
//!   The `QueueWorker` transport reads `ProcessorMeta` from the fully-assembled
//!   container and runs one apalis worker per processor.
//! - **Producer**: inject [`QueueConnection`] and call
//!   `.of::<Job>("name").push(job).await?`.
//!
//! Connection is async, so it is seeded at the composition root as a factory
//! — apalis types never leak into apps.

mod config;
mod connection;
mod module;
mod processor;
mod worker;

pub use config::QueueConfig;
pub use connection::{Queue, QueueConnection};
pub use module::{QueueModule, QueueSetup};
pub use processor::{Job, Processor, ProcessorMeta};
pub use worker::QueueWorker;

#[doc(hidden)]
pub use processor::{register_worker, FromContainer};

pub use nestrs_queue_macros::processor;

pub use async_trait::async_trait;
