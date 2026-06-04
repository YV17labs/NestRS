//! OpenTelemetry for nestrs.
//!
//! [`OpenTelemetry::init`] sets up `tracing` (console fmt always; OTLP exporter when the
//! `otlp` feature is on and `NESTRS_OPENTELEMETRY__OTLP_ENDPOINT` is set). The returned
//! guard flushes on drop, so it must outlive `main`.
//!
//! [`OpenTelemetryModule`] activates the HTTP interceptor: `traceparent` propagation,
//! per-request span, status recording, `X-Trace-Id` response header, and one
//! access event per request (gated by `NESTRS_HTTP__ACCESS_LOG`).

mod config;
mod error;
mod init;
#[cfg(feature = "http")]
mod interceptor;
mod module;
#[cfg(feature = "otlp")]
mod otlp;

pub use config::{LogFormat, OpenTelemetryConfig};
pub use error::OpenTelemetryError;
pub use init::OpenTelemetry;
#[cfg(feature = "otlp")]
pub use module::OpenTelemetryMeter;
pub use module::OpenTelemetryModule;
