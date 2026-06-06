//! Redis-backed queue integration for nestrs — the first-class backend
//! that plugs into the abstractions defined by
//! [`nestrs-queue`](::nestrs_queue) (the [`Job`] marker, the [`Processor`]
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
//! `nestrs-<storage>` crate against the same `nestrs-queue` abstractions;
//! the macro and application code stay unchanged.
//!
//! ## Future expansion
//!
//! If Redis grows a second nestrs use (cache, pub/sub, distributed locks),
//! this crate adds matching Cargo feature flags rather than spawning a
//! sibling crate — Redis is one external dependency, this is its one
//! integration home.
//!
//! [`Job`]: ::nestrs_queue::Job
//! [`Processor`]: ::nestrs_queue::Processor
//! [`ProcessMethod`]: ::nestrs_queue::ProcessMethod

mod config;
mod connection;
mod module;
mod worker;
mod worker_module;

pub use config::QueueConfig;
pub use connection::{Queue, QueueConnection};
pub use module::{QueueModule, QueueSetup};
pub use worker::QueueWorker;
pub use worker_module::QueueWorkerModule;
