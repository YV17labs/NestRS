//! Worker-execution primitives shared by every transport that runs jobs off the
//! request path — schedulers, queue workers, future stream consumers.
//!
//! A worker transport runs work that no client is actively awaiting, so it has
//! no HTTP request to hang ambient state from. The seam in this crate
//! ([`JobContext`]) lets a bridge (e.g. an ORM module) install per-job ambient
//! state — a pool executor, a tenant scope, a trace span — without coupling the
//! worker transport to that bridge's domain.

pub mod context;

pub use context::{JobContext, run_in_job_context};
