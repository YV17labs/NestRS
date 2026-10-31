//! OpenTelemetry for nestrs.
//!
//! [`OpenTelemetry::init`] sets up `tracing` — the console always, and an OTLP
//! exporter for each signal whose endpoint the OpenTelemetry specification's
//! variables set (`OTEL_EXPORTER_OTLP_ENDPOINT`) and whose selector does not
//! switch it off (`OTEL_TRACES_EXPORTER=none`), under the `otlp` feature —
//! with the service named after the binary `#[nest_rs::main]` started. The
//! returned guard flushes on drop, so it must outlive `main`.
//!
//! That line is the whole wiring: no module to import, nothing for a suite to
//! call. Under the `otlp` feature, `OpenTelemetryModule` provides `Metrics`
//! to the module whose provider injects them, where an app records its
//! measurements through instruments named by the OpenTelemetry Metrics API —
//! `Counter`, `UpDownCounter`, `Histogram`, `Gauge` — with no `opentelemetry`
//! type in its signatures. The remote parent link and the sampler's verdict
//! are seeded onto the framework's span constructor at `init`, so they reach
//! **every** edge.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — The exporter's own diagnostics.
pub const TARGET: &str = "nest_rs::opentelemetry";

#[cfg(feature = "otlp")]
mod attribute;
mod config;
mod error;
#[cfg(feature = "otlp")]
mod id_generator;
mod init;
#[cfg(feature = "otlp")]
mod instruments;
#[cfg(feature = "otlp")]
mod linker;
#[cfg(feature = "otlp")]
mod metrics;
#[cfg(feature = "otlp")]
mod module;
#[cfg(feature = "otlp")]
mod otlp;
mod propagators;
mod signal;
#[cfg(test)]
mod testing;

#[cfg(feature = "otlp")]
pub use attribute::{Attribute, AttributeValue};
pub use config::{DEFAULT_METRIC_INTERVAL, OpenTelemetryConfig};
pub use error::OpenTelemetryError;
pub use init::{FLUSH_TIMEOUT, OpenTelemetry};
#[cfg(feature = "otlp")]
pub use instruments::{Counter, Gauge, Histogram, UpDownCounter};
#[cfg(feature = "otlp")]
pub use metrics::{Describe, Metrics};
#[cfg(feature = "otlp")]
pub use module::OpenTelemetryModule;
