//! The open queue contract for nestrs.
//!
//! `nest-rs-queue` defines **what every queue backend must agree on**: a
//! queue's identity ([`Queue`], [`QueueName`]) and the payload bound ([`Job`]);
//! the `#[processor]` inventory ([`ProcessMethod`]) and what one attempt at a job
//! is ([`consume`]); the push surface ([`JobProducerExt`]) over the seam a backend
//! enqueues and removes jobs through ([`JobProducer`]); and the capability model a
//! backend declares what it honours with ([`QueueBackend`], [`Capability`]).
//!
//! An application reaches all of it through the umbrella — `nest_rs::queue` —
//! and a backend's own types through that backend's module: `nest_rs::redis` for
//! the first-party Redis backend. A third-party backend depends on this crate.
//!
//! # Extension contract
//!
//! A backend is a `nest-rs-<storage>` crate that implements the two seams below
//! and ships the three module shapes every adapter has (`<Vendor>Module::for_root`
//! opening the connection, `<Vendor>QueueModule` binding the producer,
//! `<Vendor>WorkerModule::for_root` contributing the worker transport).
//!
//! 1. **Declare the backend once**: a `const` [`QueueBackend`] carrying the
//!    backend's name (its `messaging.system`) and the [`Capabilities`] it honours.
//!    Retries, one transaction per attempt and per-method concurrency are not
//!    capabilities, because no backend may refuse them — but of the three, only
//!    concurrency is yours to honour: the port counts the retry budget and times
//!    its backoff, and the worker's `JobContext` owns the transaction. See
//!    [`Capability`].
//! 2. **File jobs**: implement [`JobProducer`] — `backend` returns the constant,
//!    `enqueue` stores sealed [`Envelope`]s, each carrying the [`JobId`] the port
//!    minted, which is the job's key everywhere the backend keeps something about
//!    it. The port has already refused every option the backend does not declare,
//!    and answers the caller with the [`PushReceipt`]s itself. A backend declaring
//!    [`Capability::Cancellation`] implements `remove` too, and `remove_unique`
//!    beside [`Capability::UniquePush`].
//!    Bind it as `Arc<dyn JobProducer>` with
//!    `ContainerBuilder::provide_declared_factory_after`, carrying
//!    [`BACKEND_REMEDY`]: **two backends imported is a boot error naming both**.
//! 3. **Run jobs**: a `Transport` whose `configure` calls
//!    [`consume::discover`] with the constant — which refuses, at boot, every
//!    `#[process]` declaration the backend lacks — and whose fetch loop builds a
//!    [`Delivery`](consume::Delivery) per job and calls [`consume::attempt`],
//!    translating the [`AttemptOutcome`](consume::AttemptOutcome): `Ok` into its
//!    acknowledgement, `DeadLetter` into its dead letter, and `Retry { after }`
//!    into the next attempt once `after` has passed — re-filing
//!    [`Delivery::retry_envelope`](consume::Delivery::retry_envelope) when it
//!    declares [`Capability::DelayedPush`], waiting in process and calling
//!    `attempt` again when it does not. It honours each method's `concurrency`,
//!    and opens no span of its own: the attempt, its span, its events, its budget
//!    and its backoff are the port's.
//!
//! A value the backend's storage cannot hold — a name its own rules refuse, a
//! window finer than it keeps — is refused by the backend, naming that fact.
//! The walkthrough is the docs' *Writing a queue driver*.

#![warn(missing_docs)]

/// This crate's span target — job registration, attempts, dead-letters and the
/// reason each failed.
///
/// Declared by the crate that **owns** the concern, which is not always the only
/// crate emitting on it: a sibling and a `*-macros` expansion read this constant
/// rather than spelling a second one, because a target's one job is to say
/// **where** an event came from.
pub const TARGET: &str = "nest_rs::queue";

mod backend;
mod backoff;
mod capability;
mod checkpoint;
mod destination;
mod error;
mod inventory;
mod job;
mod job_id;
mod process_options;
mod producer;
mod push_options;
mod push_receipt;
mod queue;
mod queue_name;
pub mod unit;

/// The port's half of consuming: discovery and one attempt at a job.
pub mod consume;
// The wire envelope a job travels in, sealed and opened by this crate alone. A
// private module with its type re-exported flat, like every other type here:
// `seal` and `open` are `pub(crate)`, so the module path reached nothing a
// caller may call and only offered `Envelope` and `WIRE_FORMAT_VERSION` a second
// spelling. `consume` and `unit` stay `pub mod` because their principal exports
// are a procedure and a constant, read as `consume::attempt` and `unit::JOB`.
mod envelope;

pub use backend::{BACKEND_REMEDY, QueueBackend};
pub use capability::{Capabilities, Capability};
pub use checkpoint::{Checkpoint, CheckpointStore};
// `CheckpointCell` is the type of a `pub` field on the exported `HandlerContext`,
// so it is nameable whether or not it is re-exported — and unnameable is the
// worse of the two. Hidden beside its peers, never shown.
#[doc(hidden)]
pub use checkpoint::CheckpointCell;
pub use destination::Destination;
pub use envelope::{Envelope, WIRE_FORMAT_VERSION};
pub use error::{JobError, QueueError};
pub use inventory::ProcessMethod;
#[doc(hidden)]
pub use inventory::{HandlerContext, JobHandler};
pub use job::Job;
pub use job_id::JobId;
pub use process_options::{ProcessOptions, Throttle};
pub use producer::{JobProducer, JobProducerExt};
pub use push_options::{Delay, PushOptions};
pub use push_receipt::PushReceipt;
pub use queue::{DynamicQueue, Queue, QueueInstance, QueueKind};
pub use queue_name::{INSTANCE_SEPARATOR, QueueName};

// Re-export `async_trait` so backends implement the async traits this crate
// defines without depending on it directly.
pub use async_trait::async_trait;

// `#[processor]`-generated code names `::nest_rs_queue::serde_json::*`, so this
// crate re-exports it — keeping the macro free of any dependency the call site
// would have to declare.
#[doc(hidden)]
pub use serde_json;

// Re-exported for `#[processor]`-generated code, which runs every handler inside
// the ambient `JobContext` a worker transport installs — so writing a processor
// never requires naming `nest-rs-worker` in the call site's manifest.
#[doc(hidden)]
pub use nest_rs_worker;

/// The wire-DTO shorthand — same decorator the HTTP layer uses, re-exported
/// here so a payload crossing this transport needs no `serde` of its own.
pub use nest_rs_core::input;

pub use nest_rs_queue_macros::{processor, queue};
