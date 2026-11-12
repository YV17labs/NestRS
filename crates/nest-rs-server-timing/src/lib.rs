//! W3C [Server-Timing] interceptor for nestrs.
//!
//! Importing [`ServerTimingModule`] adds a `Server-Timing` header to every
//! response but a `401`, `403`, `407` or `429` (browsers render the cost in
//! their Network panel) in the development and test profiles; a deployment
//! elsewhere turns it on through [`ServerTimingConfig`]. Handlers record
//! sub-step durations by pulling [`Timings`] out of request extensions.
//!
//! [Server-Timing]: https://www.w3.org/TR/server-timing/
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — the boot line of a header turned on outside
/// development.
pub const TARGET: &str = "nest_rs::server_timing";

mod config;
mod entry;
mod format;
mod interceptor;
mod module;

pub use config::ServerTimingConfig;
pub use entry::{Entry, Timings};
pub use module::{ServerTimingModule, ServerTimingSetup};
