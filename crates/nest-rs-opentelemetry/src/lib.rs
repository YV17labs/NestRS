//! OpenTelemetry for nestrs.
//!
//! [`OpenTelemetry::init`] sets up `tracing` (console fmt always; OTLP exporter when the
//! `otlp` feature is on and `NESTRS_OPENTELEMETRY__OTLP_ENDPOINT` is set). The returned
//! guard flushes on drop, so it must outlive `main`.
//!
//! [`OpenTelemetryModule`] provides the OTel meter. Everything else this crate
//! adds — the remote parent link and the sampler's verdict — is seeded onto the
//! framework's span constructor at `init`, so it reaches **every** edge rather
//! than the one transport an interceptor could hang from.
//!
//! The span, the W3C trace context and the access log belong to the transports
//! and to `nest-rs-core`: they must exist whether or not this crate does.

#![warn(missing_docs)]

/// This crate's span target — The exporter's own diagnostics.
///
/// Declared by the crate that **owns** the concern, which is not always the only
/// crate emitting on it: a sibling and a `*-macros` expansion read this constant
/// rather than spelling a second one, because a target's one job is to say
/// **where** an event came from. A central table in the kernel would have meant
/// `nest-rs-core` holding a name for a concern it does not know exists.
pub const TARGET: &str = "nest_rs::opentelemetry";

mod config;
mod error;
#[cfg(feature = "otlp")]
mod id_generator;
mod init;
#[cfg(feature = "otlp")]
mod linker;
mod module;
#[cfg(feature = "otlp")]
mod otlp;

pub use config::{DEFAULT_METRIC_INTERVAL, LogFormat, OpenTelemetryConfig};
pub use error::OpenTelemetryError;
pub use init::OpenTelemetry;
#[cfg(feature = "otlp")]
pub use module::OpenTelemetryMeter;
pub use module::OpenTelemetryModule;
