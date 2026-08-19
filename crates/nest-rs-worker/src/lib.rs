//! Worker-execution primitives shared by every transport that runs jobs off the
//! request path — schedulers, queue workers, future stream consumers.
//!
//! A worker transport runs work that no client is actively awaiting, so it has
//! no HTTP request to hang ambient state from. The seam in this crate
//! ([`JobContext`]) lets a bridge (e.g. an ORM module) install per-job ambient
//! state — an executor (by default a transaction settled on the job's own
//! outcome, see [`JobTransaction`]), a tenant scope, a trace span — without
//! coupling the worker transport to that bridge's domain.
//!
//! Vocabulary: *worker* = transport role (drives execution off the request
//! path); *job* = unit of work executed. [`JobContext`] is per-**job** ambient
//! state, not per-worker — installed once around each unit of work the
//! transport drives.
#![warn(missing_docs)]

pub mod context;

/// This crate's span target, at the root like every other crate's — the module
/// it is declared in is an implementation detail of where the emission lives.
pub use context::TARGET;

pub use context::{JobContext, JobSettlement, JobTransaction, Unhonoured, run_in_job_context};
