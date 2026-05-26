//! W3C [Server-Timing] interceptor for nestrs.
//!
//! Adds a `Server-Timing` response header so Chrome DevTools (and every
//! other modern browser) renders per-request server cost natively in the
//! Network panel. Independent of OpenTelemetry — this is purely a W3C HTTP
//! concern.
//!
//! ```ignore
//! use nestrs_core::module;
//! use nestrs_server_timing::ServerTimingModule;
//!
//! #[module(imports = [ServerTimingModule])]
//! pub struct AppModule;
//! ```
//!
//! Handlers can record sub-step durations by pulling the [`Timings`]
//! accumulator out of request extensions and calling [`Timings::record`].
//!
//! [Server-Timing]: https://www.w3.org/TR/server-timing/

mod entry;
mod format;
mod interceptor;
mod module;

pub use entry::{Entry, Timings};
pub use module::ServerTimingModule;
