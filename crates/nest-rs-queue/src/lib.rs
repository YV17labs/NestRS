//! The open queue contract for nestrs.
//!
//! `nest-rs-queue` defines **what every queue backend must agree on**: the
//! [`Job`] marker, the [`Processor`] trait, the [`ProcessMethod`] inventory
//! entry the `#[processor]` macro submits, and the [`JobProducer`] seam a
//! backend implements to enqueue.
//!
//! The first-class backend is **Redis** (via apalis-redis), shipped as
//! `nest-rs-redis`. Application code keeps writing `nest_rs_queue::*` for the
//! abstractions — the `#[processor]` macro, `Job`, `Processor`,
//! `ProcessMethod`, `JobProducer` — and reaches for `nest_rs_redis::*` only
//! when it needs the Redis-specific types (the `RedisQueueProducer`,
//! the `RedisWorker` transport, the activation modules). A third-party
//! `nest-rs-<storage>` (e.g. SQS, NATS, in-memory) depends on this crate
//! directly — see this crate's README for the extension contract.

#![warn(missing_docs)]

/// This crate's span target — Job registration, retries, dead-letters and the reason each failed.
///
/// Declared by the crate that **owns** the concern, which is not always the only
/// crate emitting on it: a sibling and a `*-macros` expansion read this constant
/// rather than spelling a second one, because a target's one job is to say
/// **where** an event came from. A central table in the kernel would have meant
/// `nest-rs-core` holding a name for a concern it does not know exists.
pub const TARGET: &str = "nest_rs::queue";

mod error;
mod inventory;
mod processor;
mod producer;
mod queue_name;
pub mod unit;

// The wire envelope, both halves. `pub` because a backend crate
// (`nest-rs-redis`, or a third-party one) is what pushes and drains, so it is
// what seals and opens; this crate owns the *shape* so every backend wraps
// identically. A seam between framework crates, not a surface an app calls.
pub mod consume;
pub mod envelope;
pub use error::QueueError;
pub use inventory::{
    JobError, JobHandler, ProcessMethod, WIRE_FORMAT_VERSION, check_duplicate_queue_claims,
};
pub use processor::{Job, Processor};
pub use producer::{JobProducer, JobProducerExt};
pub use queue_name::QueueName;

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

// Re-exported for `#[processor]`-generated code, which runs every handler
// inside the ambient `JobContext` a worker transport installs. Same reason as
// the two above: `nest_rs_queue` stays the single import a `#[process]` method
// needs, so writing a processor never requires naming `nest-rs-worker` in the
// call site's manifest.
#[doc(hidden)]
pub use nest_rs_worker;

/// The wire-DTO shorthand — same decorator the HTTP layer uses, re-exported
/// here so a payload crossing this transport needs no `serde` of its own.
pub use nest_rs_core::input;

pub use nest_rs_queue_macros::{processor, queue};
