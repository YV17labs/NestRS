//! The open queue contract for nestrs.
//!
//! `nest-rs-queue` defines **what every queue backend must agree on**: the
//! [`Job`] marker, the [`Processor`] trait, the [`ProcessMethod`] inventory
//! entry the `#[processor]` macro submits, and the three pluggable seams a
//! backend implements — [`QueueBackend`], [`JobProducer`], [`JobConsumer`].
//!
//! The first-class backend is **Redis** (via apalis-redis), shipped as
//! `nest-rs-redis`. Application code keeps writing `nest_rs_queue::*` for the
//! abstractions — the `#[processor]` macro, `Job`, `Processor`,
//! `ProcessMethod`, `JobProducer` — and reaches for `nest_rs_redis::*` only
//! when it needs the Redis-specific types (the `QueueConnection` producer,
//! the `QueueWorker` transport, the activation modules). A third-party
//! `nest-rs-<storage>` (e.g. SQS, NATS, in-memory) depends on this crate
//! directly — see this crate's README for the extension contract.
mod consumer;
mod inventory;
mod processor;
mod producer;

pub use consumer::JobConsumer;
pub use inventory::{JobHandler, ProcessMethod, ProcessorMeta, WIRE_FORMAT_VERSION};
pub use processor::{FromContainer, Job, Processor};
pub use producer::{JobProducer, JobProducerExt, QueueBackend};

// Re-export `async_trait` so backends and macros don't need to depend on it
// directly to implement the async traits this crate defines.
pub use async_trait::async_trait;

// The `inventory::collect!` lives in `inventory.rs` — the registry is the
// open seam between the `#[process]` macro emission and any backend that
// drains it at boot.

// `#[processor]`-generated code names `::nest_rs_queue::ProcessMethod`,
// `::nest_rs_queue::JobHandler`, and `::nest_rs_queue::serde_json::*`, so this
// crate re-exports both the macro and `serde_json` — keeping the macro free
// of any backend dependency and letting the call site reach the macro
// through `nest_rs_queue::processor` regardless of which backend integration
// (nest-rs-redis, …) the app imports.
#[doc(hidden)]
pub use serde_json;

// Re-exported for `#[processor]`-generated code that emits a `warn!` for
// unversioned legacy payloads. Keeps the macro free of any extra dependency
// at the call site.
#[doc(hidden)]
pub use tracing;

pub use nest_rs_queue_macros::processor;
