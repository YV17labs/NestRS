//! Telemetry for nestrs.
//!
//! [`Telemetry::init`] sets up `tracing` (console fmt always; OTLP exporter
//! when the `otlp` feature is on and `NESTRS_TELEMETRY__OTLP_ENDPOINT` is
//! set). The returned guard flushes on drop, so it must outlive `main`.
//!
//! [`TelemetryModule`] activates the HTTP interceptor: `traceparent`
//! propagation, per-request span, status recording, `X-Trace-Id` response
//! header, and one access event per request (gated by
//! `NESTRS_HTTP__ACCESS_LOG`).

mod config;
mod error;
#[cfg(feature = "http")]
mod interceptor;
mod module;
#[cfg(feature = "otlp")]
mod otlp;
mod telemetry;

pub use config::{LogFormat, TelemetryConfig};
pub use error::TelemetryError;
#[cfg(feature = "otlp")]
pub use module::Meter;
pub use module::TelemetryModule;
pub use telemetry::Telemetry;
