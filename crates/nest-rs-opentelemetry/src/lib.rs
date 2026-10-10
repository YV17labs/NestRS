//! OpenTelemetry for nestrs.
//!
//! [`OpenTelemetry::init`] sets up `tracing` (console fmt always; OTLP exporter when the
//! `otlp` feature is on and `<PREFIX>_OPENTELEMETRY__OTLP_ENDPOINT` is set). The returned
//! guard flushes on drop, so it must outlive `main`.
//!
//! [`OpenTelemetryModule`] provides the OTel meter. The remote parent link and
//! the sampler's verdict are seeded onto the framework's span constructor at
//! `init`, so they reach **every** edge.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — The exporter's own diagnostics.
pub const TARGET: &str = "nest_rs::opentelemetry";

mod config;
mod error;
#[cfg(feature = "otlp")]
mod id_generator;
mod init;
#[cfg(feature = "otlp")]
mod linker;
#[cfg(feature = "otlp")]
mod meter;
mod module;
#[cfg(feature = "otlp")]
mod otlp;

pub use config::{DEFAULT_METRIC_INTERVAL, LogFormat, OpenTelemetryConfig};
pub use error::OpenTelemetryError;
pub use init::{FLUSH_TIMEOUT, OpenTelemetry};
#[cfg(feature = "otlp")]
pub use meter::OpenTelemetryMeter;
pub use module::OpenTelemetryModule;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::init::init_for_tests;
}
