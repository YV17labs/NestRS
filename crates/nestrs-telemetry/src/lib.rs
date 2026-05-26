//! Telemetry for nestrs applications.
//!
//! Single entry point: [`Telemetry::init`] sets up `tracing` (console fmt
//! always; OTLP exporter when the `otlp` feature is on and
//! `NESTRS_TELEMETRY__OTLP_ENDPOINT` is set). The returned [`Telemetry`]
//! guard flushes pending spans on drop, so it must outlive `main`'s work.
//!
//! All runtime knobs live behind the `NESTRS_<DOMAIN>__<KEY>` env-var
//! scheme — see [`TelemetryConfig`] for the full table.
//!
//! [`TelemetryModule`] is the entry point — import it to activate the HTTP
//! interceptor (`OtelHttp`, crate-private): it bridges incoming W3C
//! `traceparent` headers into per-request `tracing` spans, records the
//! response status, surfaces the trace id as `X-Trace-Id` on responses, and
//! — when the `NESTRS_HTTP__ACCESS_LOG` toggle is on — emits one
//! `tracing::info!` event per request with the htaccess-style summary.
//!
//! Sibling HTTP middleware lives in its own crate when it does not drive
//! OTel export:
//! - `nestrs-server-timing` — W3C Server-Timing response header.

mod config;
mod error;
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
