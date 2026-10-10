//! The open queue contract for nestrs.
//!
//! `nest-rs-queue` defines **what every queue backend must agree on**: a
//! queue's identity ([`Queue`], [`QueueName`]) and the payload bound ([`Job`]);
//! the `#[processor]` inventory ([`ProcessMethod`]) and the worker running each
//! attempt at a job ([`QueueWorker`]); the push surface ([`JobProducerExt`])
//! over the seam a backend enqueues and removes jobs through ([`JobProducer`]);
//! and the capability model a backend declares what it honours with
//! ([`QueueBackend`], [`Capability`]).
//!
//! An application reaches all of it through the umbrella — `nest_rs::queue` —
//! and a backend's own types through that backend's module: `nest_rs::redis` for
//! the first-party Redis backend. A third-party backend depends on this crate.
//!
//! # Extension contract
//!
//! A backend is a `nest-rs-<storage>` crate that implements the two seams below
//! and binds both in one module, `<Vendor>QueueModule`, beside the
//! `<Vendor>Module::for_root` opening its connection. The worker is the port's:
//! an app runs jobs by importing [`QueueModule`], which attaches the
//! [`QueueWorker`] to whichever backend is bound.
//!
//! 1. **Declare the backend once**: a `const` [`QueueBackend`] carrying the
//!    backend's name (its `messaging.system`) and the [`Capabilities`] it honours.
//!    Retries, one transaction per attempt and per-method concurrency are not
//!    capabilities and not yours to keep: the port and its worker own them. See
//!    [`Capability`].
//! 2. **File jobs**: implement [`JobProducer`] — `backend` returns the constant,
//!    `enqueue` stores sealed [`Envelope`]s, each carrying the [`JobId`] the port
//!    minted, which is the job's key everywhere the backend keeps something about
//!    it. The port has already refused every option the backend does not declare,
//!    and answers the caller with the [`PushReceipt`]s itself. A backend declaring
//!    [`Capability::Cancellation`] implements `remove` too, and `remove_unique`
//!    beside [`Capability::UniquePush`]. The port hands `enqueue`
//!    [`ENQUEUE_BATCH`] envelopes at most, and waits [`BACKEND_TIMEOUT`] for the
//!    answer to every call it makes — these three and a [`CheckpointStore`]'s —
//!    before dropping it and answering [`QueueError::Unanswered`]: bound your own
//!    round trips well inside that net, so an outage reaches the caller as your
//!    failure, with its cause.
//!    Bind it as `Arc<dyn JobProducer>` with
//!    `ContainerBuilder::provide_declared_factory_after`, carrying
//!    [`BACKEND_REMEDY`]: **two backends imported is a boot error naming both**.
//! 3. **Run jobs**: implement [`JobConsumer`] — hand over leased
//!    [`Delivery`]s, renew their leases, and end each delivery as its
//!    [`Disposition`] says, fenced on its lease — and bind it as a
//!    [`BoundConsumer`] the same way, carrying the same remedy. The worker owns
//!    everything else: discovery, which refuses at boot every `#[process]`
//!    declaration the backend lacks; each method's `concurrency`; the attempt,
//!    its span, its events, its budget and its backoff; the renewal cadence;
//!    the [`STALL_LIMIT`] on deliveries that end unanswered; and the drain. A
//!    backend not declaring [`Capability::DelayedPush`] is never handed a wait
//!    to keep: the worker waits in process. `nest_rs_testing::queue_kit!` runs
//!    the behaviour every backend owes the worker — a worker dying between
//!    receive and settle, a lease taken under a running attempt, the drain —
//!    against yours.
//!
//! **Delivery is at least once, on every backend.** A storage that loses no job
//! delivers some twice — a lease lapsed after a crash, an acknowledgement lost —
//! and none promises more: a `#[process]` handler is idempotent.
//!
//! A value the backend's storage cannot hold — a name its own rules refuse, a
//! window finer than it keeps — is refused by the backend, naming that fact.
//! The walkthrough is the docs' *Writing a queue driver*.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — job registration, attempts, dead-letters and the
/// reason each failed.
pub const TARGET: &str = "nest_rs::queue";

mod backend;
mod backoff;
mod capability;
mod checkpoint;
mod config;
mod consume;
mod consumer;
mod delivery;
mod destination;
mod disposition;
mod envelope;
mod error;
mod inventory;
mod job;
mod job_id;
mod module;
mod process_options;
mod producer;
mod push_options;
mod push_receipt;
mod queue;
mod queue_name;
pub mod unit;
mod worker;

pub use backend::{BACKEND_REMEDY, BACKEND_TIMEOUT, QueueBackend};
pub use capability::{Capabilities, Capability};
pub use checkpoint::{Checkpoint, CheckpointStore};
pub use config::QueueConfig;
pub use consume::{NEWER_RELEASE_PATIENCE, NEWER_RELEASE_WAIT, STALL_LIMIT};
pub use consumer::{Ask, BoundConsumer, JobConsumer, LeaseHold, Prepared, Received};
pub use delivery::Delivery;
pub use destination::Destination;
pub use disposition::Disposition;
pub use envelope::{Envelope, WIRE_FORMAT_VERSION};
pub use error::{JobError, QueueError};
pub use inventory::ProcessMethod;
pub use job::Job;
pub use job_id::JobId;
pub use module::{QueueModule, QueueSetup};
pub use process_options::{ProcessOptions, Throttle};
pub use producer::{ENQUEUE_BATCH, JobProducer, JobProducerExt};
pub use push_options::{Delay, PushOptions};
pub use push_receipt::PushReceipt;
pub use queue::Queue;
pub use queue_name::QueueName;
pub use worker::{QueueWorker, lease_fits_renewal};

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    /// What one attempt at a job is, run by [`QueueWorker`](crate::QueueWorker);
    /// reached here so this crate's suite drives an attempt without a worker.
    pub mod consume {
        pub use crate::consume::{AttemptOutcome, Delivery, attempt, discover, refuse};
    }

    pub use crate::checkpoint::{CheckpointCell, open_checkpoint};
    pub use crate::error::undecodable;
    pub use crate::inventory::{
        HandlerContext, JobHandler, decode, process_method, required_capabilities,
    };
    pub use crate::queue_name::is_valid_queue_name;

    pub use nest_rs_worker;
    pub use serde_json;
}

// Backends implement this crate's async traits without depending on it.
pub use async_trait::async_trait;

/// The wire-DTO shorthand, so a payload crossing this transport needs no `serde`
/// of its own.
pub use nest_rs_core::input;

/// Orchestrator on an `#[injectable]` provider's `impl` block. Each method
/// tagged with `#[process(queue = <Queue>, …)]` becomes a queue consumer — a
/// [`ProcessMethod`] — that a backend's worker transport drains at boot.
///
/// ```
/// use nest_rs_core::injectable;
/// use nest_rs_queue::{ProcessMethod, Queue, input, processor, queue};
///
/// #[input]
/// #[derive(Clone)]
/// pub struct TranscodeCommand {
///     pub file: String,
/// }
///
/// #[queue(name = "audio", job = TranscodeCommand)]
/// pub struct AudioQueue;
///
/// #[injectable]
/// #[derive(Default)]
/// pub struct AudioProcessor;
///
/// #[processor]
/// impl AudioProcessor {
///     #[process(queue = AudioQueue, retries = 3)]
///     async fn transcode(&self, job: TranscodeCommand) -> anyhow::Result<()> {
///         anyhow::ensure!(!job.file.is_empty(), "nothing to transcode");
///         Ok(())
///     }
/// }
///
/// let transcode = nest_rs_core::inventory::iter::<ProcessMethod>()
///     .find(|method| method.name() == "AudioProcessor::transcode");
/// assert_eq!(transcode.map(ProcessMethod::queue), Some(<AudioQueue as Queue>::NAME));
/// assert_eq!(transcode.map(|method| method.options().retries()), Some(3));
/// ```
pub use nest_rs_queue_macros::processor;

/// Stamp a unit struct with a queue's compile-time identity — its wire name and
/// the `Job` payload it carries — by implementing [`Queue`]. The producer
/// (`queue.push(Q, job, None)`) and the consumer (`#[process(queue = Q)]`) both
/// name the type, so a typo'd name or a mismatched payload is a compile error.
///
/// ```
/// # use std::any::TypeId;
/// # use nest_rs_queue::{Destination, Queue, input, queue};
/// # #[input]
/// # #[derive(Clone)]
/// # pub struct TranscodeCommand {
/// #     pub file: String,
/// # }
/// // The marker is the destination a push names.
/// #[queue(name = "audio", job = TranscodeCommand)]
/// pub struct AudioQueue;
///
/// assert_eq!(<AudioQueue as Queue>::NAME, "audio");
/// assert_eq!(TypeId::of::<<AudioQueue as Queue>::Job>(), TypeId::of::<TranscodeCommand>());
/// assert_eq!(AudioQueue.queue_name()?.as_str(), "audio");
/// # Ok::<(), nest_rs_queue::QueueError>(())
/// ```
pub use nest_rs_queue_macros::queue;
