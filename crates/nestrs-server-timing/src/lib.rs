//! W3C [Server-Timing] interceptor for nestrs.
//!
//! Importing [`ServerTimingModule`] adds a `Server-Timing` header to every
//! response (browsers render the cost in their Network panel). Handlers record
//! sub-step durations by pulling [`Timings`] out of request extensions.
//!
//! [Server-Timing]: https://www.w3.org/TR/server-timing/

mod entry;
mod format;
mod interceptor;
mod module;

pub use entry::{Entry, Timings};
pub use module::ServerTimingModule;
