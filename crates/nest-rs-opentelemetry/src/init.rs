use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

use std::sync::atomic::{AtomicBool, Ordering};

use nest_rs_core::logging::{JsonFormat, TextFormat};

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
    /// Reads `<PREFIX>_OPENTELEMETRY__*`. Batch exporters are added only when an OTLP
    /// endpoint is set, but the tracer is always installed so `trace_id` and
    /// `traceparent` propagation work out of the box.
    pub fn init(service_name: impl Into<String>) -> Result<Self, OpenTelemetryError> {
        Self::init_with(OpenTelemetryConfig::from_env(service_name)?)
    }

    /// Console-only init for tests. Idempotent; first call wins. No flush
    /// guard. Log level honours `<PREFIX>_LOG` then `RUST_LOG`, default `warn`; an
    /// invalid directive falls through rather than failing a test run.
    #[doc(hidden)]
    #[expect(
        clippy::disallowed_methods,
        reason = "the logging bootstrap runs before any config exists"
    )]
    pub fn init_for_tests() {
        if initialized() {
            return;
        }
        let filter = std::env::var(nest_rs_core::EnvPrefix::var(
            nest_rs_core::logging::var::FILTER,
        ))
        .ok()
        .and_then(|spec| EnvFilter::try_new(&spec).ok())
        .or_else(|| EnvFilter::try_from_default_env().ok())
        .unwrap_or_else(|| EnvFilter::new("warn"));
        #[expect(
            clippy::let_underscore_must_use,
            reason = "an Err is a subscriber already installed, which a test run keeps"
        )]
        let _ = Registry::default()
            .with(filter)
            .with(console_layer(LogFormat::Text, false))
            .try_init();
        mark_initialized();
    }

    /// Install the subscriber from an explicit [`OpenTelemetryConfig`]. Returns the
    /// flush guard that must outlive `main`, or an error that aborts boot on an
    /// unparseable filter, a metric interval outside its range or a failed exporter
    /// build.
    pub fn init_with(config: OpenTelemetryConfig) -> Result<Self, OpenTelemetryError> {
        // A config built in code reaches here without `from_env`, so its interval is
        // held to the variable's range here.
        crate::config::METRIC_INTERVAL.check(
            crate::config::NAMESPACE,
            "OpenTelemetryConfig::metric_interval",
            config.metric_interval,
        )?;
        let filter = parse_log_filter(&config.log_filter)?;
        let fmt_layer = console_layer(config.log_format, config.log_source_location);

        #[cfg(feature = "otlp")]
        {
            let exporters = crate::otlp::build(&config)?;
            let otel_layer = tracing_opentelemetry::layer().with_tracer(exporters.tracer);
            // Seeded here rather than mounted by a module: a queue worker imports no
            // transport.
            crate::linker::install();

            // Bridge only when an exporter is present; otherwise it pays the
            // per-event cost just to drop the event.
            let appender_layer = match exporters.logger_provider.as_ref() {
                Some(lp) => Some(
                    opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(lp)
                        .with_filter(parse_log_filter(&config.log_filter)?),
                ),
                None => None,
            };

            Registry::default()
                .with(filter)
                .with(fmt_layer)
                .with(otel_layer)
                .with(appender_layer)
                .try_init()
                .map_err(|e| OpenTelemetryError::Init(e.to_string()))?;
            mark_initialized();

            tracing::info!(
                target: crate::TARGET,
                service = %config.service_name,
                mode = "otlp",
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
                target: crate::TARGET,
                service = %config.service_name,
                mode = "console",
                log_format = ?config.log_format,
                "OpenTelemetry initialised"
            );
            Ok(OpenTelemetry {})
        }
    }
}

/// Parse an `EnvFilter` directive string, mapping a rejection to a named,
/// boot-aborting error instead of falling back to `info`.
fn parse_log_filter(spec: &str) -> Result<EnvFilter, OpenTelemetryError> {
    EnvFilter::try_new(spec).map_err(|source| OpenTelemetryError::InvalidLogFilter {
        value: spec.to_owned(),
        source,
    })
}

/// Boxed because `text` and `json` layers have distinct concrete types.
/// `FmtSpan::NONE`: a span's lifecycle is not an event; the console shows the
/// operation line each edge files on `nest_rs::operation`.
fn console_layer<S>(
    format: LogFormat,
    source_location: bool,
) -> Box<dyn Layer<S> + Send + Sync + 'static>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    match format {
        // The kernel's formatters, the ones the fallback subscriber installs: a line
        // cannot change shape because the app adopted an exporter. See
        // `nest_rs_core::logging::TextFormat` for why `with_file` / `with_line_number`
        // are absent here.
        LogFormat::Text => tracing_subscriber::fmt::layer()
            .event_format(TextFormat::new(source_location))
            .boxed(),
        LogFormat::Json => tracing_subscriber::fmt::layer()
            .json()
            .event_format(JsonFormat::new(source_location))
            .boxed(),
    }
}

/// How long the final telemetry flush may hold the exit, every provider at once.
///
/// The last bounded step of the way down, after the transports' windows and the
/// shutdown hooks' budget (`nest_rs_core::SHUTDOWN_HOOKS_TIMEOUT` tabulates the
/// sum, under the 30 s a Kubernetes pod is given before `SIGKILL`).
///
/// **The providers flush concurrently, each on a thread of its own**: the SDK's
/// `shutdown` blocks up to five seconds per provider. What is still exporting at
/// the bound is abandoned and said on stderr, naming the provider.
pub const FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

impl Drop for OpenTelemetry {
    fn drop(&mut self) {
        #[cfg(feature = "otlp")]
        self.flush_within(FLUSH_TIMEOUT);
    }
}

#[cfg(feature = "otlp")]
impl OpenTelemetry {
    /// Shut every provider down at once, waiting `bound` for all three
    /// together — [`FLUSH_TIMEOUT`] from `Drop`.
    #[expect(
        clippy::print_stderr,
        reason = "the final flush runs after the subscriber it would log through is gone"
    )]
    fn flush_within(&mut self, bound: std::time::Duration) {
        type Shutdown = Box<dyn FnOnce() -> opentelemetry_sdk::error::OTelSdkResult + Send>;

        // `Drop` can't return, and tracing may itself be mid-teardown, so
        // every failure here goes to stderr directly. A failed or abandoned
        // final flush loses telemetry, and says so.
        let deadline = std::time::Instant::now() + bound;
        let mut flushes: Vec<(&'static str, Shutdown)> = Vec::new();
        if let Some(p) = self.tracer_provider.take() {
            flushes.push(("tracer", Box::new(move || p.shutdown_with_timeout(bound))));
        }
        // The metrics provider ignores the timeout it is handed and waits
        // its reader's fixed five seconds (opentelemetry_sdk 0.33), which is
        // why the bound is enforced here rather than trusted to the SDK.
        if let Some(p) = self.meter_provider.take() {
            flushes.push(("meter", Box::new(move || p.shutdown_with_timeout(bound))));
        }
        if let Some(p) = self.logger_provider.take() {
            flushes.push(("logger", Box::new(move || p.shutdown_with_timeout(bound))));
        }

        let (done, settled) = std::sync::mpsc::channel();
        let mut pending: Vec<&'static str> = Vec::new();
        for (provider, shutdown) in flushes {
            let done = done.clone();
            let spawned = std::thread::Builder::new()
                    .name(format!("otel-flush-{provider}"))
                    .spawn(move || {
                        #[expect(clippy::let_underscore_must_use, reason = "the receiver stops listening at the deadline, which it reports itself")]
                        let _ = done.send((provider, shutdown()));
                    });
            match spawned {
                Ok(_) => pending.push(provider),
                Err(e) => eprintln!(
                    "{}: {provider} provider not flushed: no thread to flush it on: {e}",
                    crate::TARGET,
                ),
            }
        }
        drop(done);

        while !pending.is_empty() {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match settled.recv_timeout(left) {
                Ok((provider, outcome)) => {
                    pending.retain(|p| *p != provider);
                    if let Err(e) = outcome {
                        eprintln!(
                            "{}: {provider} provider shutdown failed: {e}",
                            crate::TARGET
                        );
                    }
                }
                Err(_) => {
                    eprintln!(
                        "{}: final flush abandoned after {} ms, still exporting: {}",
                        crate::TARGET,
                        bound.as_millis(),
                        pending.join(", "),
                    );
                    break;
                }
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
        // Restored after: `init_for_tests` depends on the prior state.
        let prior = INITIALIZED.swap(false, Ordering::Relaxed);
        assert!(!initialized());
        mark_initialized();
        assert!(initialized());
        INITIALIZED.store(prior, Ordering::Relaxed);
    }

    #[test]
    fn console_layer_produces_a_text_layer_for_text_format() {
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
        let layer = console_layer::<Registry>(LogFormat::Text, true);
        let _subscriber = Registry::default().with(layer);
    }

    #[test]
    fn init_for_tests_is_idempotent() {
        let _guard = INIT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        OpenTelemetry::init_for_tests();
        assert!(initialized(), "init_for_tests must flip the global flag");
        OpenTelemetry::init_for_tests();
        assert!(initialized());
    }

    #[test]
    fn init_with_rejects_a_set_but_unparseable_log_filter() {
        let config = OpenTelemetryConfig::new("svc").with_log_filter("foo=notalevel");
        // `OpenTelemetry` has no `Debug`, so no `expect_err`.
        match OpenTelemetry::init_with(config) {
            Err(OpenTelemetryError::InvalidLogFilter { value, .. }) => {
                assert_eq!(
                    value, "foo=notalevel",
                    "the error must name the bad directive"
                );
            }
            Err(other) => panic!("expected InvalidLogFilter, got {other:?}"),
            Ok(_) => panic!("a set-but-unparseable filter must abort init, not degrade to `info`"),
        }
    }

    #[test]
    fn parse_log_filter_accepts_a_valid_directive() {
        assert!(parse_log_filter("debug,hyper=warn").is_ok());
    }

    /// A collector that never answers holds the exit for one bound, the meter's
    /// provider included, which ignores the one it is handed.
    #[cfg(feature = "otlp")]
    #[test]
    fn a_collector_that_never_answers_holds_the_final_flush_to_the_bound_for_all_three_providers() {
        use opentelemetry::logs::{LogRecord as _, Logger as _, LoggerProvider as _};
        use opentelemetry::metrics::MeterProvider as _;
        use opentelemetry::trace::{Tracer as _, TracerProvider as _};
        use opentelemetry_otlp::{
            LogExporter, MetricExporter, Protocol, SpanExporter, WithExportConfig,
        };

        // Bound and never accepted: the kernel completes the handshake, so each
        // export's request goes out and its answer never comes.
        let collector = std::net::TcpListener::bind("127.0.0.1:0").expect("a local port");
        let base = format!("http://{}", collector.local_addr().expect("its address"));

        let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(
                SpanExporter::builder()
                    .with_http()
                    .with_endpoint(format!("{base}/v1/traces"))
                    .with_protocol(Protocol::HttpBinary)
                    .build()
                    .expect("the span exporter builds"),
            )
            .build();
        tracer_provider.tracer("pin").in_span("queued", |_| {});

        let meter_provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
            .with_reader(
                opentelemetry_sdk::metrics::PeriodicReader::builder(
                    MetricExporter::builder()
                        .with_http()
                        .with_endpoint(format!("{base}/v1/metrics"))
                        .with_protocol(Protocol::HttpBinary)
                        .build()
                        .expect("the metric exporter builds"),
                )
                .build(),
            )
            .build();
        meter_provider
            .meter("pin")
            .u64_counter("queued")
            .build()
            .add(1, &[]);

        let logger_provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
            .with_batch_exporter(
                LogExporter::builder()
                    .with_http()
                    .with_endpoint(format!("{base}/v1/logs"))
                    .with_protocol(Protocol::HttpBinary)
                    .build()
                    .expect("the log exporter builds"),
            )
            .build();
        let logger = logger_provider.logger("pin");
        let mut record = logger.create_log_record();
        record.set_body("queued".into());
        logger.emit(record);

        let mut guard = OpenTelemetry {
            tracer_provider: Some(tracer_provider),
            meter_provider: Some(meter_provider),
            logger_provider: Some(logger_provider),
        };
        let bound = std::time::Duration::from_millis(200);
        let started = std::time::Instant::now();
        guard.flush_within(bound);
        let took = started.elapsed();

        assert!(
            took >= bound && took < bound + std::time::Duration::from_secs(1),
            "the three providers were held to one bound between them, took {took:?}",
        );
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
