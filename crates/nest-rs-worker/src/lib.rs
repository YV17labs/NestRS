//! Worker-execution primitives shared by every transport that runs jobs off the
//! request path — schedulers, queue workers, future stream consumers.
//!
//! [`JobContext`] lets a bridge (e.g. an ORM module) install per-job ambient
//! state — an executor (by default a transaction settled on the job's own
//! outcome, see [`JobTransaction`]), a tenant scope — without coupling the
//! worker transport to that bridge's domain.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod constants;
pub mod context;
mod error;

/// This crate's span target.
pub use context::TARGET;

pub use constants::JOB_TIMEOUT;
pub use context::{BACKEND_REMEDY, JobContext, JobSettlement, JobTransaction, run_in_job_context};
pub use error::{JobTimedOut, Unhonoured};
