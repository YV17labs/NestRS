#[cfg(feature = "otlp")]
use tracing::Dispatch;
#[cfg(feature = "otlp")]
use tracing_subscriber::Layer as _;
use tracing_subscriber::Registry;
#[cfg(feature = "otlp")]
use tracing_subscriber::filter::FilterExt as _;
use tracing_subscriber::layer::SubscriberExt;

use nest_rs_core::__private::{ConsoleLayer, install_global_subscriber, log_filter};

use crate::config::{OpenTelemetryConfig, say_retired_variables};
use crate::error::OpenTelemetryError;

const ALREADY_INSTALLED: &str = "a global subscriber is already installed, and is kept";

/// What the boot says when a deployment exports a signal over plain HTTP off
/// this machine: honoured, since its `http://` endpoint wrote it.
#[cfg(feature = "otlp")]
const PLAINTEXT_EXPORT: &str = "a signal is exported over plain HTTP outside a dev profile: its \
     telemetry travels unencrypted";

/// Drop synchronously flushes pending traces/metrics/logs. Keep the binding
/// alive for the whole program: `let _otel = OpenTelemetry::init()?;`.
#[must_use = "dropping the guard shuts the exporters down at once: bind it, \
              `let _otel = OpenTelemetry::init()?;`, for the whole of `main`"]
pub struct OpenTelemetry {
    #[cfg(feature = "otlp")]
    tracer_provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
    #[cfg(feature = "otlp")]
    meter_provider: Option<opentelemetry_sdk::metrics::SdkMeterProvider>,
    #[cfg(feature = "otlp")]
    logger_provider: Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
}

impl OpenTelemetry {
    /// Install the subscriber from the environment
    /// ([`OpenTelemetryConfig::from_env`]), the service named after the binary
    /// `#[nest_rs::main]` started. A signal's batch exporter is added only when
    /// an OTLP endpoint is set for it (`OTEL_EXPORTER_OTLP_ENDPOINT`) and
    /// neither its selector (`OTEL_TRACES_EXPORTER`, `_METRICS_`, `_LOGS_`) nor
    /// `OTEL_SDK_DISABLED` turns it off, but the tracer is always installed so
    /// `trace_id` and `traceparent` propagation work out of the box.
    pub fn init() -> Result<Self, OpenTelemetryError> {
        Self::init_with(OpenTelemetryConfig::from_env()?)
    }

    /// Install the subscriber from `config`, under the variables that override
    /// it field by field: the OpenTelemetry specification's (`OTEL_*`) and the
    /// console's (`RUST_LOG`, `<PREFIX>_LOG_FORMAT`,
    /// `<PREFIX>_LOG_SOURCE_LOCATION`), as [`OpenTelemetryConfig::from_env`]
    /// reads them. Returns the flush guard that must outlive `main`, or an
    /// error that aborts boot on a filter, a setting, a number the SDK reads
    /// or a header that does not parse, a batch processor's queue or batch
    /// size it cannot run with, an export no timeout bounds, a service name
    /// naming nothing, a request this binary cannot serve —
    /// an exporter, a compression or a propagator it does not build,
    /// `OTEL_CONFIG_FILE`, userinfo in an endpoint, a query on a base
    /// endpoint, a header beside a plaintext endpoint on another host or a
    /// query in one, an endpoint in a binary built without the `otlp`
    /// feature — a metric interval or sample ratio outside its range, or a
    /// failed exporter build. A specification variable this build serves
    /// nowhere is said once at `warn`, unless its value names what it does, and
    /// so, in a deployed profile, is each signal exporting over `http://` off
    /// this machine.
    ///
    /// The framework propagates W3C Trace Context at every edge and carries a
    /// queue job's W3C Baggage onto the pushes it makes, whatever this crate
    /// installs, so `OTEL_PROPAGATORS` may only name those two.
    pub fn init_with(config: OpenTelemetryConfig) -> Result<Self, OpenTelemetryError> {
        let console = config.console()?;
        let filter = log_filter(&console.filter);
        // The boot line says what the console writes: `format` may hold `ByProfile`.
        let log_format = if console.format.writes_json() {
            "json"
        } else {
            "text"
        };

        #[cfg(feature = "otlp")]
        {
            let started = std::time::Instant::now();
            let exporters = crate::otlp::build(&config)?;

            let appender_layer = exporters
                .logger_provider
                .as_ref()
                .map(opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new);
            // Every layer takes the log filter, the trace export every unit's
            // span besides and the durations every unit declaring one, whatever
            // the console writes: filters agreeing on a callsite the log filter
            // settles keep it settled, where disagreeing ones would ask it on
            // every event. A `None` layer would answer every callsite, so the
            // durations' arm is the other's with one layer more.
            let logs = ConsoleLayer::new(console.format, console.source_location)
                .and_then(appender_layer)
                .with_filter(filter);
            let unit_spans = crate::unit_spans::UnitSpans::new(exporters.traces_export);
            let traces = tracing_opentelemetry::layer()
                .with_tracer(exporters.tracer.clone())
                .with_filter(log_filter(&console.filter).or(unit_spans));
            let subscriber = match exporters.meter_provider.as_ref() {
                None => Dispatch::new(Registry::default().with(logs).with(traces)),
                Some(provider) => Dispatch::new(Registry::default().with(logs).with(traces).with(
                    crate::unit_durations::UnitDurations::new(provider).with_filter(
                        log_filter(&console.filter).or(crate::unit_durations::MeasuredUnits),
                    ),
                )),
            };
            if !install_global_subscriber(subscriber) {
                return Err(OpenTelemetryError::Init(ALREADY_INSTALLED.to_owned()));
            }
            // Nothing global is seeded before the subscriber is the process's:
            // a refused `init` drops its exporters, and nothing records into them.
            exporters.seat_globals();
            if let Some(provider) = &exporters.meter_provider {
                nest_rs_core::__private::set_unit_end(crate::unit_durations::end_unit);
                let meter = crate::durations::framework_meter(provider);
                crate::process_metrics::ProcessMetrics::observe(&meter, started);
                crate::tokio_metrics::TokioMetrics::observe(&meter);
                crate::connection_pool_metrics::ConnectionPoolMetrics::observe(&meter);
            }
            // Seeded here rather than mounted by a module: a queue worker imports no
            // transport.
            crate::linker::install();
            crate::client_operations::install(
                exporters
                    .traces_export
                    .then_some(&exporters.tracer_provider),
                exporters.meter_provider.as_ref(),
            );

            say_retired_variables();
            let summary = exporters.summary;
            for over in summary.passed_over {
                tracing::warn!(
                    target: crate::TARGET,
                    variable = over.variable,
                    reason = over.reason,
                    "an OpenTelemetry variable is passed over",
                );
            }
            if let Some(environment) = nest_rs_config::Surface::Open.opened_in_deployment() {
                for (signal, endpoint) in summary.in_plaintext {
                    tracing::warn!(
                        target: crate::TARGET,
                        environment = environment.as_str(),
                        signal = signal.name(),
                        endpoint = endpoint.as_str(),
                        "{PLAINTEXT_EXPORT}",
                    );
                }
            }
            // Strings, which the console quotes, so a logfmt reader takes
            // `off (no endpoint)` whole.
            let [traces, metrics, logs] = summary.destinations.map(|to| to.to_string());
            tracing::info!(
                target: crate::TARGET,
                service = summary.service.as_str(),
                mode = "otlp",
                traces = traces.as_str(),
                metrics = metrics.as_str(),
                logs = logs.as_str(),
                sampler = summary.sampler.name,
                sampler_source = summary.sampler.source,
                sample_ratio = summary.sampler.ratio,
                log_format,
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
            refuse_what_no_exporter_serves(&config, &nest_rs_config::env_var)?;
            let service = console_service(
                &config,
                nest_rs_core::AppIdentity::current(),
                &nest_rs_config::env_var,
            )?;
            if !install_global_subscriber(
                Registry::default()
                    .with(filter)
                    .with(ConsoleLayer::new(console.format, console.source_location)),
            ) {
                return Err(OpenTelemetryError::Init(ALREADY_INSTALLED.to_owned()));
            }
            say_retired_variables();
            tracing::info!(
                target: crate::TARGET,
                service = service.as_str(),
                mode = "console",
                log_format,
                "OpenTelemetry initialised"
            );
            Ok(OpenTelemetry {})
        }
    }
}

/// The service a binary built without the `otlp` feature names on its boot
/// line, as the exporters' resource would name it: `OTEL_SERVICE_NAME`, then
/// `service.name` in `OTEL_RESOURCE_ATTRIBUTES`, then the pin, then the binary,
/// then [`unknown_service`]. What names none is refused as it is there. `read`
/// is the environment as written: a blank `OTEL_SERVICE_NAME` is refused, never
/// read as unset.
#[cfg(not(feature = "otlp"))]
fn console_service(
    config: &OpenTelemetryConfig,
    binary: Option<&nest_rs_core::AppIdentity>,
    read: &impl Fn(&str) -> Option<String>,
) -> Result<String, nest_rs_config::ConfigError> {
    use crate::config::{SERVICE_NAME, resource_attributes, service_name};

    let pinned = config.pinned_service_name()?;
    let attributes = resource_attributes(read)?;
    Ok(service_name(read)?
        .or_else(|| {
            attributes
                .into_iter()
                .rev()
                .find_map(|(key, value)| (key == SERVICE_NAME).then_some(value))
        })
        .or_else(|| pinned.map(str::to_owned))
        .or_else(|| binary.map(|binary| binary.name.clone()))
        .unwrap_or_else(unknown_service))
}

/// The resource specification's name for a service nothing names, as the SDK's
/// detector writes it where this build does not link one: `unknown_service:` and
/// the executable's name, or `unknown_service` where that does not read.
#[cfg(not(feature = "otlp"))]
fn unknown_service() -> String {
    use crate::config::UNKNOWN_SERVICE;

    std::env::current_exe()
        .ok()
        .and_then(|path| Some(format!("{UNKNOWN_SERVICE}:{}", path.file_name()?.to_str()?)))
        .unwrap_or_else(|| UNKNOWN_SERVICE.to_owned())
}

/// What a binary built without the `otlp` feature cannot honour, refused
/// naming the variable or the field, never the value: the specification's
/// file configuration, a propagator the framework does not speak, a selector
/// naming an exporter, and an endpoint set for a signal its selector and
/// `OTEL_SDK_DISABLED` leave on — a signal's own variable, then
/// `OTEL_EXPORTER_OTLP_ENDPOINT`, then the pinned one — since the binary
/// builds no exporter and the deployment would believe it ships.
#[cfg(not(feature = "otlp"))]
fn refuse_what_no_exporter_serves(
    config: &OpenTelemetryConfig,
    read: &impl Fn(&str) -> Option<String>,
) -> Result<(), nest_rs_config::ConfigError> {
    use crate::signal::{OTEL_EXPORTER_OTLP_ENDPOINT, SdkSwitch, Selection, Signal};

    let read = crate::config::blank_as_unset(read);
    crate::config::refuse_config_file(&read)?;
    crate::propagators::read_propagators(&read)?;
    let disabled = SdkSwitch::read(&read) == SdkSwitch::Off;
    for signal in Signal::ALL {
        if signal.selection(&read)? == Selection::None || disabled {
            continue;
        }
        let set_by = [signal.endpoint_var(), OTEL_EXPORTER_OTLP_ENDPOINT]
            .into_iter()
            .find(|variable| read(variable).is_some())
            .or_else(|| {
                config
                    .otlp_endpoint
                    .is_some()
                    .then_some(crate::config::PINNED_ENDPOINT)
            });
        if let Some(set_by) = set_by {
            return Err(nest_rs_config::ConfigError::parse(
                set_by,
                format!(
                    "sets an endpoint for {}, but this binary was built without the otlp feature \
                     and exports nothing — build it with the feature (the umbrella's \
                     opentelemetry turns it on), or turn the signal off with {}=none",
                    signal.name(),
                    signal.exporter_var(),
                ),
            ));
        }
    }
    Ok(())
}

/// How long the final telemetry flush may hold the exit, every provider at once.
///
/// A bounded step of the way down, after the transports' windows and the
/// shutdown hooks' budget and before the console's last lines
/// (`nest_rs_core::SHUTDOWN_HOOKS_TIMEOUT` tabulates the sum, under the 30 s a
/// Kubernetes pod is given before `SIGKILL`).
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
    /// together — [`FLUSH_TIMEOUT`] from `Drop`; the providers still exporting
    /// at the bound, which it abandoned.
    #[expect(
        clippy::print_stderr,
        reason = "the final flush runs after the subscriber it would log through is gone"
    )]
    fn flush_within(&mut self, bound: std::time::Duration) -> Vec<&'static str> {
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
        pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(feature = "otlp"))]
    use crate::testing::reading;

    /// A filter pinned in code is checked by the kernel's parser, which names
    /// the field and never quotes the directive.
    #[test]
    fn init_with_refuses_a_pinned_filter_that_does_not_parse_naming_the_field() {
        let config = OpenTelemetryConfig {
            log_filter: "billing=notalevel".into(),
            ..OpenTelemetryConfig::default()
        };
        // `OpenTelemetry` has no `Debug`, so no `expect_err`.
        let Err(refused) = OpenTelemetry::init_with(config) else {
            panic!("a filter that does not parse aborts init, never degrades to `info`");
        };
        assert!(
            matches!(refused, OpenTelemetryError::LogSettings(_)),
            "{refused:?}"
        );
        let said = refused.to_string();
        assert!(said.contains("OpenTelemetryConfig::log_filter"), "{said}");
        assert!(!said.contains("notalevel"), "{said}");
        assert!(!said.contains("billing"), "{said}");
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
        let abandoned = guard.flush_within(bound);
        let took = started.elapsed();

        assert!(
            took >= bound,
            "the providers were given the bound: took {took:?}"
        );
        // The meter's reader holds its shutdown five seconds whatever it is
        // handed: abandoned, it was not waited out.
        assert!(
            abandoned.contains(&"meter"),
            "the bound is held here, not trusted to the SDK: abandoned {abandoned:?}",
        );
    }

    /// The refusal of an endpoint `set` names, in a binary built without the
    /// `otlp` feature, as said.
    #[cfg(not(feature = "otlp"))]
    fn refused(config: &OpenTelemetryConfig, set: &[(&str, &str)]) -> String {
        refuse_what_no_exporter_serves(config, &reading(set))
            .expect_err("an endpoint nothing exports to")
            .to_string()
    }

    /// The binary's name stands where nothing else names the service, and the
    /// last of two `service.name`s wins, as the resource holds them.
    #[cfg(not(feature = "otlp"))]
    #[test]
    fn without_the_otlp_feature_the_service_is_named_as_the_resource_would_name_it() {
        let binary = nest_rs_core::__private::package_identity("auth", "7.0.0", "");
        let pinned = OpenTelemetryConfig {
            service_name: Some("pinned".into()),
            ..OpenTelemetryConfig::default()
        };
        let default = OpenTelemetryConfig::default();
        for (config, set, named) in [
            (&default, vec![], "auth"),
            (&pinned, vec![], "pinned"),
            (
                &pinned,
                vec![(
                    "OTEL_RESOURCE_ATTRIBUTES",
                    "service.name=one,service.name=two",
                )],
                "two",
            ),
            (
                &pinned,
                vec![
                    ("OTEL_SERVICE_NAME", "named"),
                    ("OTEL_RESOURCE_ATTRIBUTES", "service.name=one"),
                ],
                "named",
            ),
        ] {
            assert_eq!(
                console_service(config, Some(&binary), &reading(&set)).expect("a name"),
                named,
                "{set:?}"
            );
        }
    }

    #[cfg(not(feature = "otlp"))]
    #[test]
    fn without_the_otlp_feature_otel_exporter_otlp_endpoint_is_refused_naming_it() {
        let said = refused(
            &OpenTelemetryConfig::default(),
            &[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318")],
        );
        assert!(
            said.contains("OTEL_EXPORTER_OTLP_ENDPOINT")
                && said.contains("for traces")
                && said.contains("without the otlp feature"),
            "{said}"
        );
        assert!(!said.contains("collector"), "{said}");
    }

    #[cfg(not(feature = "otlp"))]
    #[test]
    fn without_the_otlp_feature_the_traces_endpoint_is_refused_naming_it() {
        let said = refused(
            &OpenTelemetryConfig::default(),
            &[(
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                "http://collector:4318/v1/traces",
            )],
        );
        assert!(
            said.contains("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") && said.contains("for traces"),
            "{said}"
        );
    }

    #[cfg(not(feature = "otlp"))]
    #[test]
    fn without_the_otlp_feature_the_metrics_endpoint_is_refused_naming_it() {
        let said = refused(
            &OpenTelemetryConfig::default(),
            &[(
                "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
                "http://collector:4318/v1/metrics",
            )],
        );
        assert!(
            said.contains("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT") && said.contains("for metrics"),
            "{said}"
        );
    }

    #[cfg(not(feature = "otlp"))]
    #[test]
    fn without_the_otlp_feature_the_logs_endpoint_is_refused_naming_it() {
        let said = refused(
            &OpenTelemetryConfig::default(),
            &[(
                "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
                "http://collector:4318/v1/logs",
            )],
        );
        assert!(
            said.contains("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT") && said.contains("for logs"),
            "{said}"
        );
    }

    #[cfg(not(feature = "otlp"))]
    #[test]
    fn without_the_otlp_feature_a_pinned_endpoint_is_refused_naming_the_field() {
        let said = refused(
            &OpenTelemetryConfig {
                otlp_endpoint: Some("http://collector:4318".into()),
                ..OpenTelemetryConfig::default()
            },
            &[],
        );
        assert!(
            said.contains("OpenTelemetryConfig::otlp_endpoint") && said.contains("for traces"),
            "{said}"
        );
        assert!(!said.contains("collector"), "{said}");
    }

    /// A signal its selector or `OTEL_SDK_DISABLED` switches off exports nothing,
    /// as the binary does: its endpoint is no refusal, while another signal's
    /// still is.
    #[cfg(not(feature = "otlp"))]
    #[test]
    fn without_the_otlp_feature_an_endpoint_switched_off_boots() {
        let pinned = OpenTelemetryConfig {
            otlp_endpoint: Some("http://collector:4318".into()),
            ..OpenTelemetryConfig::default()
        };
        assert!(
            refuse_what_no_exporter_serves(
                &pinned,
                &reading(&[
                    ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318"),
                    ("OTEL_TRACES_EXPORTER", "none"),
                    ("OTEL_METRICS_EXPORTER", "none"),
                    ("OTEL_LOGS_EXPORTER", "NONE"),
                ])
            )
            .is_ok()
        );
        assert!(
            refuse_what_no_exporter_serves(&pinned, &reading(&[("OTEL_SDK_DISABLED", "true")]))
                .is_ok()
        );
        let said = refused(
            &OpenTelemetryConfig::default(),
            &[
                ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318"),
                ("OTEL_TRACES_EXPORTER", "none"),
            ],
        );
        assert!(
            said.contains("OTEL_EXPORTER_OTLP_ENDPOINT") && said.contains("for metrics"),
            "{said}"
        );
        assert!(
            refuse_what_no_exporter_serves(&OpenTelemetryConfig::default(), &reading(&[])).is_ok()
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
