use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::{LogFormat, OpenTelemetryConfig};
use crate::error::OpenTelemetryError;

/// `OpenTelemetryModule` reads this to fail fast rather than register no-op providers
/// when `OpenTelemetry::init` was forgotten.
static INITIALIZED: AtomicBool = AtomicBool::new(false);

pub(crate) fn initialized() -> bool {
    INITIALIZED.load(Ordering::Relaxed)
}

fn mark_initialized() {
    INITIALIZED.store(true, Ordering::Relaxed);
}

/// Drop synchronously flushes pending traces/metrics/logs. Keep the binding
/// alive for the whole program: `let _otel = OpenTelemetry::init("api")?;`.
pub struct OpenTelemetry {
    #[cfg(feature = "otlp")]
    tracer_provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
    #[cfg(feature = "otlp")]
    meter_provider: Option<opentelemetry_sdk::metrics::SdkMeterProvider>,
    #[cfg(feature = "otlp")]
    logger_provider: Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
}

impl OpenTelemetry {
    /// Reads `NESTRS_OPENTELEMETRY__*`. Batch exporters are added only when an OTLP
    /// endpoint is set, but the tracer is always installed so `trace_id` and
    /// `traceparent` propagation work out of the box.
    pub fn init(service_name: impl Into<String>) -> Result<Self, OpenTelemetryError> {
        Self::init_with(OpenTelemetryConfig::from_env(service_name))
    }

    /// Console-only init for tests. Idempotent; first call wins. No flush
    /// guard. Log level honours `RUST_LOG`, default `warn`.
    pub fn init_for_tests() {
        if initialized() {
            return;
        }
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
        let _ = Registry::default()
            .with(filter)
            .with(console_layer(LogFormat::Text, false))
            .try_init();
        mark_initialized();
    }

    pub fn init_with(config: OpenTelemetryConfig) -> Result<Self, OpenTelemetryError> {
        let filter =
            EnvFilter::try_new(&config.log_filter).unwrap_or_else(|_| EnvFilter::new("info"));
        let fmt_layer = console_layer(config.log_format, config.log_source_location);

        #[cfg(feature = "otlp")]
        {
            let exporters = crate::otlp::build(&config)?;
            let otel_layer = tracing_opentelemetry::layer().with_tracer(exporters.tracer);

            // Bridge only when an exporter is present; otherwise it pays the
            // per-event cost just to drop the event.
            let appender_layer = exporters.logger_provider.as_ref().map(|lp| {
                let f = EnvFilter::try_new(&config.log_filter)
                    .unwrap_or_else(|_| EnvFilter::new("info"));
                opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(lp)
                    .with_filter(f)
            });

            Registry::default()
                .with(filter)
                .with(fmt_layer)
                .with(otel_layer)
                .with(appender_layer)
                .try_init()
                .map_err(|e| OpenTelemetryError::Init(e.to_string()))?;
            mark_initialized();

            tracing::info!(
                service = %config.service_name,
                endpoint = config.otlp_endpoint.as_deref().unwrap_or("<none>"),
                sample_ratio = config.trace_sample_ratio,
                log_format = ?config.log_format,
                otlp_export = exporters.meter_provider.is_some(),
                "OpenTelemetry initialised"
            );

            Ok(OpenTelemetry {
                tracer_provider: Some(exporters.tracer_provider),
                meter_provider: exporters.meter_provider,
                logger_provider: exporters.logger_provider,
            })
        }

        #[cfg(not(feature = "otlp"))]
        {
            Registry::default()
                .with(filter)
                .with(fmt_layer)
                .try_init()
                .map_err(|e| OpenTelemetryError::Init(e.to_string()))?;
            mark_initialized();
            tracing::info!(
                service = %config.service_name,
                log_format = ?config.log_format,
                "OpenTelemetry initialised (console only)"
            );
            Ok(OpenTelemetry {})
        }
    }
}

/// Boxed because `text` and `json` layers have distinct concrete types.
/// `FmtSpan::NONE` by default — only the explicit access-log event shows up
/// in the console.
fn console_layer<S>(
    format: LogFormat,
    source_location: bool,
) -> Box<dyn Layer<S> + Send + Sync + 'static>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    match format {
        LogFormat::Text => tracing_subscriber::fmt::layer()
            .with_file(source_location)
            .with_line_number(source_location)
            .boxed(),
        LogFormat::Json => tracing_subscriber::fmt::layer()
            .with_file(source_location)
            .with_line_number(source_location)
            .json()
            .with_current_span(false)
            .with_span_list(false)
            .boxed(),
    }
}

impl Drop for OpenTelemetry {
    fn drop(&mut self) {
        #[cfg(feature = "otlp")]
        {
            if let Some(p) = self.tracer_provider.take() {
                let _ = p.shutdown();
            }
            if let Some(p) = self.meter_provider.take() {
                let _ = p.shutdown();
            }
            if let Some(p) = self.logger_provider.take() {
                let _ = p.shutdown();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `INITIALIZED` is a process-wide `AtomicBool`; tests that mutate it must
    /// serialize so an interleaved check does not see another test's write.
    static INIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn initialized_reflects_the_mark() {
        let _guard = INIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Snapshot, mutate, restore — leaving the flag set would poison every
        // other init test in the file (and `init_for_tests` depends on it
        // returning the prior state).
        let prior = INITIALIZED.swap(false, Ordering::Relaxed);
        assert!(!initialized());
        mark_initialized();
        assert!(initialized());
        INITIALIZED.store(prior, Ordering::Relaxed);
    }

    #[test]
    fn console_layer_produces_a_text_layer_for_text_format() {
        // The returned layer is type-erased (`Box<dyn Layer<_>>`); compose it
        // against a registry to confirm the branch builds a working layer.
        let layer = console_layer::<Registry>(LogFormat::Text, false);
        let _subscriber = Registry::default().with(layer);
    }

    #[test]
    fn console_layer_produces_a_json_layer_for_json_format() {
        let layer = console_layer::<Registry>(LogFormat::Json, false);
        let _subscriber = Registry::default().with(layer);
    }

    #[test]
    fn console_layer_builds_with_source_location_enabled() {
        // The `with_file`/`with_line_number` branch composes against a registry.
        let layer = console_layer::<Registry>(LogFormat::Text, true);
        let _subscriber = Registry::default().with(layer);
    }

    #[test]
    fn init_for_tests_is_idempotent() {
        let _guard = INIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // First call may or may not be the first across the whole test
        // binary — either way `initialized()` must be true afterwards, and a
        // second call must not panic.
        OpenTelemetry::init_for_tests();
        assert!(initialized(), "init_for_tests must flip the global flag");
        // Second call hits the short-circuit branch.
        OpenTelemetry::init_for_tests();
        assert!(initialized());
    }

    #[test]
    fn drop_does_not_panic_on_empty_guard() {
        #[cfg(feature = "otlp")]
        let guard = OpenTelemetry {
            tracer_provider: None,
            meter_provider: None,
            logger_provider: None,
        };
        #[cfg(not(feature = "otlp"))]
        let guard = OpenTelemetry {};
        drop(guard);
    }
}
