//! OTel SDK wiring: a local tracer and W3C propagator always, and batch OTLP
//! exporters for traces, metrics and logs over HTTP/protobuf when an endpoint is
//! set.
//!
//! **The collector is not proved at boot** (`nest_rs_core::prove`): telemetry
//! must never stop the app it observes, so an unreachable collector costs
//! exports, never the boot.

use std::time::Duration;

use nest_rs_config::{ClientTls, ConfigError, ConfigService, endpoint_shown, var_name};
use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{
    LogExporter, MetricExporter, OTEL_EXPORTER_OTLP_LOGS_TIMEOUT,
    OTEL_EXPORTER_OTLP_METRICS_TIMEOUT, OTEL_EXPORTER_OTLP_TIMEOUT,
    OTEL_EXPORTER_OTLP_TIMEOUT_DEFAULT, OTEL_EXPORTER_OTLP_TRACES_TIMEOUT, Protocol, SpanExporter,
    WithExportConfig, WithHttpConfig,
};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use opentelemetry_semantic_conventions::{
    SCHEMA_URL,
    attribute::{DEPLOYMENT_ENVIRONMENT_NAME, SERVICE_INSTANCE_ID, SERVICE_NAME, SERVICE_VERSION},
};
use uuid::Uuid;

use crate::config::OpenTelemetryConfig;
use crate::error::OpenTelemetryError;

pub(crate) struct Exporters {
    pub tracer: opentelemetry_sdk::trace::Tracer,
    pub tracer_provider: SdkTracerProvider,
    /// `Some` only when an OTLP endpoint is set.
    pub meter_provider: Option<SdkMeterProvider>,
    /// `Some` only when an OTLP endpoint is set.
    pub logger_provider: Option<SdkLoggerProvider>,
    /// The export-timeout variables passed over for not holding a whole
    /// number of milliseconds, said once a subscriber can hear it.
    pub ignored_timeouts: Vec<&'static str>,
}

pub(crate) fn build(config: &OpenTelemetryConfig) -> Result<Exporters, OpenTelemetryError> {
    global::set_text_map_propagator(TraceContextPropagator::new());

    let resource = build_resource(config);
    let sampler = Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
        config.trace_sample_ratio,
    )));

    let mut tracer_builder = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_sampler(sampler)
        // The SDK adopts the framework's ids rather than minting a second pair; see
        // `id_generator`.
        .with_id_generator(crate::id_generator::AdoptFrameworkIds::default());

    // Parsed once, whatever set it: a config built in code skipped `from_env`.
    let mut ignored_timeouts = Vec::new();
    let endpoint = match config.otlp_endpoint.as_deref() {
        Some(raw) => {
            let parsed = crate::config::otlp_endpoint(raw, &otlp_endpoint_setting())?;
            let clients = exporter_clients(&parsed, &config.tls, &mut ignored_timeouts)?;
            Some((raw.trim().trim_end_matches('/'), clients))
        }
        None => None,
    };

    if let Some((base, clients)) = &endpoint {
        let span_exporter = SpanExporter::builder()
            .with_http()
            .with_http_client(clients.traces.clone())
            .with_endpoint(format!("{}/v1/traces", base))
            .with_protocol(Protocol::HttpBinary)
            .build()
            .map_err(not_built("traces", base))?;
        tracer_builder = tracer_builder.with_batch_exporter(span_exporter);
    }

    let tracer_provider = tracer_builder.build();
    let tracer = tracer_provider.tracer(config.service_name.clone());
    global::set_tracer_provider(tracer_provider.clone());

    let (meter_provider, logger_provider) = if let Some((base, clients)) = endpoint {
        let metric_exporter = MetricExporter::builder()
            .with_http()
            .with_http_client(clients.metrics)
            .with_endpoint(format!("{}/v1/metrics", base))
            .with_protocol(Protocol::HttpBinary)
            .build()
            .map_err(not_built("metrics", base))?;
        // Explicit: the SDK's 60 s default reads as a broken metrics pipeline for a
        // full minute during setup.
        let reader = PeriodicReader::builder(metric_exporter)
            .with_interval(config.metric_interval)
            .build();
        let meter_provider = SdkMeterProvider::builder()
            .with_resource(resource.clone())
            .with_reader(reader)
            .build();
        global::set_meter_provider(meter_provider.clone());

        let log_exporter = LogExporter::builder()
            .with_http()
            .with_http_client(clients.logs)
            .with_endpoint(format!("{}/v1/logs", base))
            .with_protocol(Protocol::HttpBinary)
            .build()
            .map_err(not_built("logs", base))?;
        let logger_provider = SdkLoggerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(log_exporter)
            .build();

        (Some(meter_provider), Some(logger_provider))
    } else {
        (None, None)
    };

    Ok(Exporters {
        tracer,
        tracer_provider,
        meter_provider,
        logger_provider,
        ignored_timeouts,
    })
}

/// The OTLP endpoint's variable, and the field a config built in code sets.
fn otlp_endpoint_setting() -> String {
    format!(
        "{}, or `OpenTelemetryConfig::otlp_endpoint` in code",
        var_name(crate::config::NAMESPACE, "OTLP_ENDPOINT")
    )
}

/// How long reaching the collector (DNS, TCP and the TLS handshake) may take
/// before an export gives up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long one export of a signal may take: its `signal` variable, then
/// `OTEL_EXPORTER_OTLP_TIMEOUT`, in milliseconds, then the SDK's 10 s.
///
/// That is the SDK's own resolution, which bounds its retries, so an attempt is
/// cut where the SDK stops waiting for it. A value that is not a whole number of
/// milliseconds is passed over, as the SDK and the OpenTelemetry specification
/// pass it over, and its variable joins `ignored`.
fn export_timeout(
    signal: &'static str,
    read: impl Fn(&str) -> Option<String>,
    ignored: &mut Vec<&'static str>,
) -> Duration {
    for variable in [signal, OTEL_EXPORTER_OTLP_TIMEOUT] {
        let Some(raw) = read(variable) else {
            continue;
        };
        match raw.parse::<u64>() {
            Ok(millis) => return Duration::from_millis(millis),
            Err(_) if !ignored.contains(&variable) => ignored.push(variable),
            Err(_) => {}
        }
    }
    OTEL_EXPORTER_OTLP_TIMEOUT_DEFAULT
}

/// The HTTP client each signal's exporter sends through.
struct ExporterClients {
    traces: reqwest::blocking::Client,
    metrics: reqwest::blocking::Client,
    logs: reqwest::blocking::Client,
}

/// The exporters' HTTP clients: TLS from `tls`, no redirect followed, the
/// connection bounded and each export by its signal's [`export_timeout`], and
/// `https` alone unless the endpoint declares plaintext with `http://` — the
/// OTLP specification lets the endpoint's scheme decide, and a collector beside
/// the app is the common case.
///
/// Built on a thread of their own, as the SDK builds its own: a blocking client
/// built on a runtime's thread panics.
fn exporter_clients(
    endpoint: &url::Url,
    tls: &ClientTls,
    ignored_timeouts: &mut Vec<&'static str>,
) -> Result<ExporterClients, OpenTelemetryError> {
    let names = ConfigService::for_namespace(crate::config::NAMESPACE);
    let plaintext = match endpoint.scheme() {
        "https" => false,
        "http" => {
            tls.refuse_beside_plaintext(&names, &names.var_name("OTLP_ENDPOINT"))?;
            true
        }
        _ => {
            return Err(ConfigError::parse(
                otlp_endpoint_setting(),
                "must be an http:// or https:// URL — OTLP/HTTP is spoken over one of the two",
            )
            .into());
        }
    };
    let nest_rs_config::__private::ReqwestTls { roots, identity } =
        nest_rs_config::__private::reqwest_tls(tls, &names)?;
    let [traces, metrics, logs] = [
        OTEL_EXPORTER_OTLP_TRACES_TIMEOUT,
        OTEL_EXPORTER_OTLP_METRICS_TIMEOUT,
        OTEL_EXPORTER_OTLP_LOGS_TIMEOUT,
    ]
    .map(|signal| export_timeout(signal, nest_rs_config::env_var, ignored_timeouts));
    // reqwest builds from the process's provider, which rustls picks on its own
    // only when exactly one is compiled in.
    nest_rs_config::crypto_provider();
    #[expect(
        clippy::map_err_ignore,
        reason = "a panic's payload may quote anything; the sentence says the build panicked"
    )]
    let built = std::thread::Builder::new()
        .name("otel-http-client".into())
        .spawn(move || {
            let client = |timeout| {
                let mut builder = reqwest::blocking::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(CONNECT_TIMEOUT)
                    .timeout(timeout)
                    .https_only(!plaintext)
                    .tls_certs_only(roots.clone());
                if let Some(identity) = &identity {
                    builder = builder.identity(identity.clone());
                }
                builder.build()
            };
            Ok::<_, reqwest::Error>(ExporterClients {
                traces: client(traces)?,
                metrics: client(metrics)?,
                logs: client(logs)?,
            })
        })
        .map_err(|error| {
            OpenTelemetryError::Otlp(format!(
                "no thread to build the OTLP HTTP clients on: {}",
                nest_rs_core::error_message(&error)
            ))
        })?
        .join()
        .map_err(|_| {
            OpenTelemetryError::Otlp("building the OTLP HTTP clients panicked".to_owned())
        })?;
    built.map_err(|error| {
        OpenTelemetryError::Otlp(format!(
            "the OTLP HTTP clients for {} could not be built: {}",
            endpoint_shown(endpoint.as_str()),
            nest_rs_core::error_message(&error.without_url())
        ))
    })
}

/// An exporter the SDK would not build, said without its own error, whose
/// sentence quotes the endpoint whole.
fn not_built<'a>(
    signal: &'static str,
    base: &'a str,
) -> impl FnOnce(opentelemetry_otlp::ExporterBuildError) -> OpenTelemetryError + 'a {
    move |_| {
        OpenTelemetryError::Otlp(format!(
            "the OTLP {signal} exporter for {} could not be built",
            endpoint_shown(base)
        ))
    }
}

fn build_resource(config: &OpenTelemetryConfig) -> Resource {
    let mut attrs = vec![KeyValue::new(SERVICE_NAME, config.service_name.clone())];
    attrs.push(KeyValue::new(
        SERVICE_INSTANCE_ID,
        config
            .service_instance_id
            .clone()
            .unwrap_or_else(|| Uuid::now_v7().to_string()),
    ));
    if let Some(v) = &config.service_version {
        attrs.push(KeyValue::new(SERVICE_VERSION, v.clone()));
    }
    if let Some(d) = &config.deployment_environment {
        attrs.push(KeyValue::new(DEPLOYMENT_ENVIRONMENT_NAME, d.clone()));
    }
    // Order is load-bearing: `with_schema_url` merges *under* the detectors'
    // resource (which always supplies a `service.name`), `with_attributes` *over*
    // it, so the configured values win.
    Resource::builder()
        .with_schema_url(Vec::<KeyValue>::new(), SCHEMA_URL)
        .with_attributes(attrs)
        .build()
}

#[cfg(test)]
#[expect(
    clippy::let_underscore_must_use,
    reason = "the tests shut the providers they built down best-effort"
)]
mod tests {
    use opentelemetry::Key;

    use super::*;

    /// `build` mutates `opentelemetry::global`, so every `build()` in this file is
    /// serialized.
    static GLOBAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// `SdkProvidedResourceDetector` always supplies a `service.name` (the
    /// `unknown_service:<binary>` sentinel); the configured one must win.
    #[test]
    fn configured_service_name_wins_over_detector_sentinel() {
        let cfg = OpenTelemetryConfig::new("configured-svc");
        let r = build_resource(&cfg);
        assert_eq!(
            r.get(&Key::new(SERVICE_NAME)).map(|v| v.to_string()),
            Some("configured-svc".to_string()),
        );
    }

    #[test]
    fn resource_omits_optional_attrs_when_unset() {
        let cfg = OpenTelemetryConfig::new("svc");
        let r = build_resource(&cfg);
        assert!(r.get(&Key::new(SERVICE_VERSION)).is_none());
        assert!(r.get(&Key::new(DEPLOYMENT_ENVIRONMENT_NAME)).is_none());
    }

    #[test]
    fn resource_includes_optional_attrs_when_set() {
        let cfg = OpenTelemetryConfig::new("svc")
            .with_service_version("1.2.3")
            .with_deployment_environment("staging");
        let r = build_resource(&cfg);
        assert_eq!(
            r.get(&Key::new(SERVICE_VERSION))
                .map(|v| v.as_str().to_string()),
            Some("1.2.3".to_string()),
        );
        assert_eq!(
            r.get(&Key::new(DEPLOYMENT_ENVIRONMENT_NAME))
                .map(|v| v.as_str().to_string()),
            Some("staging".to_string()),
        );
    }

    #[test]
    fn service_instance_id_falls_back_to_a_v7_uuid_per_call() {
        let cfg = OpenTelemetryConfig::new("svc");
        let a = build_resource(&cfg)
            .get(&Key::new(SERVICE_INSTANCE_ID))
            .map(|v| v.as_str().to_string());
        let b = build_resource(&cfg)
            .get(&Key::new(SERVICE_INSTANCE_ID))
            .map(|v| v.as_str().to_string());
        let a = a.expect("instance id present");
        let b = b.expect("instance id present");
        assert!(Uuid::parse_str(&a).is_ok(), "parses as a uuid: {a}");
        assert_ne!(a, b, "each invocation mints a fresh id");
    }

    #[test]
    fn service_instance_id_is_taken_from_config_when_set() {
        let cfg = OpenTelemetryConfig {
            service_instance_id: Some("pinned-instance".into()),
            ..OpenTelemetryConfig::new("svc")
        };
        let r = build_resource(&cfg);
        assert_eq!(
            r.get(&Key::new(SERVICE_INSTANCE_ID))
                .map(|v| v.as_str().to_string()),
            Some("pinned-instance".to_string()),
        );
    }

    #[test]
    fn build_without_endpoint_installs_only_the_local_tracer() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = OpenTelemetryConfig::new("svc-no-endpoint");
        let exporters = build(&cfg).expect("build succeeds without endpoint");
        assert!(
            exporters.meter_provider.is_none(),
            "meter provider must stay None until an endpoint is set",
        );
        assert!(
            exporters.logger_provider.is_none(),
            "logger provider must stay None until an endpoint is set",
        );
        let _ = exporters.tracer_provider.shutdown();
    }

    #[test]
    fn build_with_endpoint_installs_all_three_providers() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = OpenTelemetryConfig::new("svc-with-endpoint")
            .with_otlp_endpoint("http://localhost:4318");
        let exporters = build(&cfg).expect("build succeeds with endpoint");
        assert!(
            exporters.meter_provider.is_some(),
            "an endpoint must wire the meter exporter",
        );
        assert!(
            exporters.logger_provider.is_some(),
            "an endpoint must wire the logger exporter",
        );
        let _ = exporters.tracer_provider.shutdown();
        if let Some(p) = exporters.meter_provider {
            let _ = p.shutdown();
        }
        if let Some(p) = exporters.logger_provider {
            let _ = p.shutdown();
        }
    }

    #[test]
    fn build_with_trailing_slash_endpoint_does_not_double_slash_paths() {
        // A double-slash URL is still a valid URI and the providers expose no
        // endpoint, so this pins only that a trailing-slash base still builds.
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cfg =
            OpenTelemetryConfig::new("svc-trailing").with_otlp_endpoint("http://localhost:4318/");
        let exporters = build(&cfg).expect("build succeeds with trailing slash endpoint");
        assert!(exporters.meter_provider.is_some());
        assert!(exporters.logger_provider.is_some());
        let _ = exporters.tracer_provider.shutdown();
        if let Some(p) = exporters.meter_provider {
            let _ = p.shutdown();
        }
        if let Some(p) = exporters.logger_provider {
            let _ = p.shutdown();
        }
    }

    /// The tracer provider holds its final export to the bound `Drop` hands it
    /// ([`crate::FLUSH_TIMEOUT`]), so an SDK bump that stops honouring it fails
    /// here rather than leave an abandoned flush thread exporting forever.
    #[test]
    fn a_collector_that_never_answers_holds_the_final_flush_to_the_bound_it_is_handed() {
        use opentelemetry::trace::Tracer as _;

        // Bound and never accepted: the kernel completes the handshake, so the
        // export's request goes out and its answer never comes.
        let collector = std::net::TcpListener::bind("127.0.0.1:0").expect("a local port");
        let exporter = SpanExporter::builder()
            .with_http()
            .with_endpoint(format!(
                "http://{}/v1/traces",
                collector.local_addr().expect("its address")
            ))
            .with_protocol(Protocol::HttpBinary)
            .build()
            .expect("the exporter builds");
        let provider = SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .build();
        provider.tracer("pin").in_span("queued", |_| {});

        let bound = std::time::Duration::from_millis(200);
        let started = std::time::Instant::now();
        let flushed = provider.shutdown_with_timeout(bound);
        let took = started.elapsed();

        assert!(
            flushed.is_err(),
            "the final export never answered, so the flush cannot have succeeded",
        );
        assert!(
            took >= bound && took < bound + std::time::Duration::from_secs(1),
            "shutdown gave the export its bound and no more, took {took:?}",
        );
    }

    /// OTLP/HTTP is spoken over http or https: any other scheme is refused
    /// naming the variable, before an exporter is built.
    #[test]
    fn an_endpoint_neither_http_nor_https_is_refused_naming_its_variable() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = OpenTelemetryConfig::new("svc-grpc").with_otlp_endpoint("grpc://collector:4317");
        let Err(refused) = build(&cfg) else {
            panic!("a gRPC endpoint has no HTTP exporter");
        };
        let sentence = refused.to_string();
        assert!(
            sentence.contains(&var_name("opentelemetry", "OTLP_ENDPOINT"))
                && sentence.contains("must be an http:// or https:// URL"),
            "{sentence}"
        );
    }

    #[test]
    fn build_with_an_https_endpoint_installs_all_three_providers() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = OpenTelemetryConfig::new("svc-https")
            .with_otlp_endpoint("https://collector.example:4318");
        let exporters = build(&cfg).expect("build succeeds with an https endpoint");
        assert!(exporters.meter_provider.is_some());
        assert!(exporters.logger_provider.is_some());
        let _ = exporters.tracer_provider.shutdown();
        if let Some(p) = exporters.meter_provider {
            let _ = p.shutdown();
        }
        if let Some(p) = exporters.logger_provider {
            let _ = p.shutdown();
        }
    }

    /// The SDK's order: the signal's variable, then the general one, in
    /// milliseconds, then its 10 s; a value that is not a whole number of
    /// milliseconds is passed over and named once.
    #[test]
    fn an_export_timeout_is_resolved_as_the_sdk_resolves_it() {
        let reading = |set: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                set.iter()
                    .find(|(variable, _)| *variable == name)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        let resolved = |set: &'static [(&'static str, &'static str)]| {
            let mut ignored = Vec::new();
            let timeouts = [
                OTEL_EXPORTER_OTLP_TRACES_TIMEOUT,
                OTEL_EXPORTER_OTLP_METRICS_TIMEOUT,
                OTEL_EXPORTER_OTLP_LOGS_TIMEOUT,
            ]
            .map(|signal| export_timeout(signal, reading(set), &mut ignored));
            (timeouts.map(|t| t.as_millis()), ignored)
        };

        assert_eq!(resolved(&[]), ([10_000; 3], vec![]));
        assert_eq!(
            resolved(&[(OTEL_EXPORTER_OTLP_TIMEOUT, "2000")]),
            ([2000; 3], vec![])
        );
        assert_eq!(
            resolved(&[
                (OTEL_EXPORTER_OTLP_TIMEOUT, "2000"),
                (OTEL_EXPORTER_OTLP_METRICS_TIMEOUT, "300"),
            ]),
            ([2000, 300, 2000], vec![])
        );
        assert_eq!(
            resolved(&[
                (OTEL_EXPORTER_OTLP_TIMEOUT, "2000"),
                (OTEL_EXPORTER_OTLP_LOGS_TIMEOUT, "5s"),
            ]),
            ([2000, 2000, 2000], vec![OTEL_EXPORTER_OTLP_LOGS_TIMEOUT])
        );
        assert_eq!(
            resolved(&[(OTEL_EXPORTER_OTLP_TIMEOUT, "-1")]),
            ([10_000; 3], vec![OTEL_EXPORTER_OTLP_TIMEOUT])
        );
    }

    #[test]
    fn build_propagates_sample_ratio_through_resource() {
        // The sampler has no public accessor, so this pins only that a non-default
        // ratio builds.
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = OpenTelemetryConfig::new("svc-sampled").with_trace_sample_ratio(0.5);
        let exporters = build(&cfg).expect("build succeeds with custom sampler");
        let _ = exporters.tracer_provider.shutdown();
    }
}
