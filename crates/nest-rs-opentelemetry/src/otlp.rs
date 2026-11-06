//! OTel SDK wiring: a local tracer and the W3C propagators always, and a batch
//! OTLP exporter over HTTP/protobuf for each signal — traces, metrics, logs —
//! whose endpoint is set, unless its selector (`OTEL_TRACES_EXPORTER`,
//! `_METRICS_`, `_LOGS_`) says `none` or `OTEL_SDK_DISABLED` turns them all off.
//!
//! **The OpenTelemetry specification's variables configure it** (`OTEL_*`),
//! over what the app pinned in [`OpenTelemetryConfig`]: the SDK reads its
//! batch processors' numbers itself, and what it read is not overridden, but
//! a number it would pass over, or could not run with as written, is refused
//! here; the service name, the resource attributes, the
//! selectors, the endpoints, the headers, the sampler, the compression, the
//! temporality, the export timeouts, the span limits, the metric period and
//! the certificates are resolved here as the specification orders them — a
//! signal's own variable, then the general one — through the SDK's constants
//! where it exports them, so a value is refused naming its variable, never
//! quoted, and each is handed to the SDK rather than left for it to read
//! again. A specification variable this build serves nowhere is said at
//! `warn` unless its value names what the build does, and so, in a deployed
//! profile, is a signal exporting over `http://` off this machine.
//!
//! **The collector is not proved at boot** (`nest_rs_core::prove`): telemetry
//! must never stop the app it observes, so an unreachable collector costs
//! exports, never the boot.

use std::collections::HashMap;
use std::str::FromStr as _;
use std::time::Duration;

use nest_rs_config::__private::is_loopback;
use nest_rs_config::{
    ClientTls, ConfigError, Material, TlsIdentity, TlsNames, TlsSetting, endpoint_shown,
};
use nest_rs_core::{AppIdentity, Secret};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{InstrumentationScope, Key, KeyValue, global};
use opentelemetry_otlp::{
    Compression, LogExporter, MetricExporter, OTEL_EXPORTER_OTLP_COMPRESSION,
    OTEL_EXPORTER_OTLP_HEADERS, OTEL_EXPORTER_OTLP_INSECURE, OTEL_EXPORTER_OTLP_LOGS_COMPRESSION,
    OTEL_EXPORTER_OTLP_LOGS_HEADERS, OTEL_EXPORTER_OTLP_LOGS_INSECURE,
    OTEL_EXPORTER_OTLP_LOGS_PROTOCOL, OTEL_EXPORTER_OTLP_LOGS_TIMEOUT,
    OTEL_EXPORTER_OTLP_METRICS_COMPRESSION, OTEL_EXPORTER_OTLP_METRICS_HEADERS,
    OTEL_EXPORTER_OTLP_METRICS_INSECURE, OTEL_EXPORTER_OTLP_METRICS_PROTOCOL,
    OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE, OTEL_EXPORTER_OTLP_METRICS_TIMEOUT,
    OTEL_EXPORTER_OTLP_PROTOCOL, OTEL_EXPORTER_OTLP_PROTOCOL_HTTP_PROTOBUF,
    OTEL_EXPORTER_OTLP_TIMEOUT, OTEL_EXPORTER_OTLP_TIMEOUT_DEFAULT,
    OTEL_EXPORTER_OTLP_TRACES_COMPRESSION, OTEL_EXPORTER_OTLP_TRACES_HEADERS,
    OTEL_EXPORTER_OTLP_TRACES_INSECURE, OTEL_EXPORTER_OTLP_TRACES_PROTOCOL,
    OTEL_EXPORTER_OTLP_TRACES_TIMEOUT, Protocol, SpanExporter, WithExportConfig, WithHttpConfig,
};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{
    OTEL_BLRP_EXPORT_TIMEOUT, OTEL_BLRP_EXPORT_TIMEOUT_DEFAULT, OTEL_BLRP_MAX_EXPORT_BATCH_SIZE,
    OTEL_BLRP_MAX_QUEUE_SIZE, OTEL_BLRP_MAX_QUEUE_SIZE_DEFAULT, OTEL_BLRP_SCHEDULE_DELAY,
    SdkLogRecord, SdkLoggerProvider,
};
use opentelemetry_sdk::metrics::{
    OTEL_METRIC_EXPORT_INTERVAL, PeriodicReader, SdkMeterProvider, Temporality,
};
use opentelemetry_sdk::resource::{SdkProvidedResourceDetector, TelemetryResourceDetector};
use opentelemetry_sdk::trace::{
    OTEL_BSP_EXPORT_TIMEOUT, OTEL_BSP_EXPORT_TIMEOUT_DEFAULT, OTEL_BSP_MAX_EXPORT_BATCH_SIZE,
    OTEL_BSP_MAX_QUEUE_SIZE, OTEL_BSP_MAX_QUEUE_SIZE_DEFAULT, OTEL_BSP_SCHEDULE_DELAY, Sampler,
    SdkTracerProvider, SpanData, SpanLimits, TracerProviderBuilder,
};
use opentelemetry_semantic_conventions::{
    SCHEMA_URL,
    attribute::{DEPLOYMENT_ENVIRONMENT_NAME, SERVICE_INSTANCE_ID, SERVICE_VERSION},
};
use reqwest::header::{HeaderName, HeaderValue};
use uuid::Uuid;

use crate::config::{
    OpenTelemetryConfig, PINNED_ENDPOINT, PINNED_INTERVAL, PINNED_RATIO, PINNED_TLS, SERVICE_NAME,
    blank_as_unset, percent_decoded, refuse_config_file, resource_attributes, service_name,
};
use crate::error::OpenTelemetryError;
use crate::propagators::{global_propagator, read_propagators};
use crate::signal::{OTEL_EXPORTER_OTLP_ENDPOINT, OTEL_SDK_DISABLED, SdkSwitch, Selection, Signal};
#[cfg(test)]
use crate::signal::{
    OTEL_EXPORTER_OTLP_LOGS_ENDPOINT, OTEL_EXPORTER_OTLP_METRICS_ENDPOINT,
    OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
};

// opentelemetry-otlp 0.33 quotes its request's URL, whose userinfo or query may
// carry a collector's credential, and the collector's error body at `debug`.
nest_rs_core::inventory::submit! {
    nest_rs_core::__private::LibraryClamp::new(
        "opentelemetry-otlp",
        tracing::level_filters::LevelFilter::INFO,
    )
}

// The specification's names for what the SDK reads itself, or not at all;
// opentelemetry_sdk 0.33 keeps its own private.
const OTEL_TRACES_SAMPLER: &str = "OTEL_TRACES_SAMPLER";
const OTEL_TRACES_SAMPLER_ARG: &str = "OTEL_TRACES_SAMPLER_ARG";
const OTEL_METRIC_EXPORT_TIMEOUT: &str = "OTEL_METRIC_EXPORT_TIMEOUT";
const OTEL_EXPORTER_OTLP_CERTIFICATE: &str = "OTEL_EXPORTER_OTLP_CERTIFICATE";
const OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE: &str = "OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE";
const OTEL_EXPORTER_OTLP_CLIENT_KEY: &str = "OTEL_EXPORTER_OTLP_CLIENT_KEY";
const OTEL_ATTRIBUTE_COUNT_LIMIT: &str = "OTEL_ATTRIBUTE_COUNT_LIMIT";
const OTEL_ATTRIBUTE_VALUE_LENGTH_LIMIT: &str = "OTEL_ATTRIBUTE_VALUE_LENGTH_LIMIT";
const OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT: &str = "OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT";
const OTEL_SPAN_ATTRIBUTE_VALUE_LENGTH_LIMIT: &str = "OTEL_SPAN_ATTRIBUTE_VALUE_LENGTH_LIMIT";
const OTEL_SPAN_EVENT_COUNT_LIMIT: &str = "OTEL_SPAN_EVENT_COUNT_LIMIT";
const OTEL_SPAN_LINK_COUNT_LIMIT: &str = "OTEL_SPAN_LINK_COUNT_LIMIT";
const OTEL_EVENT_ATTRIBUTE_COUNT_LIMIT: &str = "OTEL_EVENT_ATTRIBUTE_COUNT_LIMIT";
const OTEL_LINK_ATTRIBUTE_COUNT_LIMIT: &str = "OTEL_LINK_ATTRIBUTE_COUNT_LIMIT";
const OTEL_LOGRECORD_ATTRIBUTE_COUNT_LIMIT: &str = "OTEL_LOGRECORD_ATTRIBUTE_COUNT_LIMIT";
const OTEL_LOGRECORD_ATTRIBUTE_VALUE_LENGTH_LIMIT: &str =
    "OTEL_LOGRECORD_ATTRIBUTE_VALUE_LENGTH_LIMIT";
const OTEL_METRICS_EXEMPLAR_FILTER: &str = "OTEL_METRICS_EXEMPLAR_FILTER";
const OTEL_EXPORTER_OTLP_METRICS_DEFAULT_HISTOGRAM_AGGREGATION: &str =
    "OTEL_EXPORTER_OTLP_METRICS_DEFAULT_HISTOGRAM_AGGREGATION";
const OTEL_LOG_LEVEL: &str = "OTEL_LOG_LEVEL";
// The SDK's own, which the specification does not name; it keeps the constant private.
const OTEL_BSP_MAX_CONCURRENT_EXPORTS: &str = "OTEL_BSP_MAX_CONCURRENT_EXPORTS";

/// The specification's default for `OTEL_METRIC_EXPORT_TIMEOUT`.
const METRIC_EXPORT_TIMEOUT_DEFAULT: Duration = Duration::from_secs(30);

/// The variables the specification names for one signal OTLP exports.
impl Signal {
    /// What it appends to a base endpoint.
    fn path(self) -> &'static str {
        match self {
            Self::Traces => "/v1/traces",
            Self::Metrics => "/v1/metrics",
            Self::Logs => "/v1/logs",
        }
    }

    fn protocol_var(self) -> &'static str {
        match self {
            Self::Traces => OTEL_EXPORTER_OTLP_TRACES_PROTOCOL,
            Self::Metrics => OTEL_EXPORTER_OTLP_METRICS_PROTOCOL,
            Self::Logs => OTEL_EXPORTER_OTLP_LOGS_PROTOCOL,
        }
    }

    fn timeout_var(self) -> &'static str {
        match self {
            Self::Traces => OTEL_EXPORTER_OTLP_TRACES_TIMEOUT,
            Self::Metrics => OTEL_EXPORTER_OTLP_METRICS_TIMEOUT,
            Self::Logs => OTEL_EXPORTER_OTLP_LOGS_TIMEOUT,
        }
    }

    fn compression_var(self) -> &'static str {
        match self {
            Self::Traces => OTEL_EXPORTER_OTLP_TRACES_COMPRESSION,
            Self::Metrics => OTEL_EXPORTER_OTLP_METRICS_COMPRESSION,
            Self::Logs => OTEL_EXPORTER_OTLP_LOGS_COMPRESSION,
        }
    }

    fn headers_var(self) -> &'static str {
        match self {
            Self::Traces => OTEL_EXPORTER_OTLP_TRACES_HEADERS,
            Self::Metrics => OTEL_EXPORTER_OTLP_METRICS_HEADERS,
            Self::Logs => OTEL_EXPORTER_OTLP_LOGS_HEADERS,
        }
    }

    /// Its processor's own bound on one export, and the specification's
    /// default for it: the batch span and log processors this crate builds
    /// apply neither, and the metric reader its own alone.
    fn processor_timeout(self) -> (&'static str, Duration) {
        match self {
            Self::Traces => (OTEL_BSP_EXPORT_TIMEOUT, OTEL_BSP_EXPORT_TIMEOUT_DEFAULT),
            Self::Metrics => (OTEL_METRIC_EXPORT_TIMEOUT, METRIC_EXPORT_TIMEOUT_DEFAULT),
            Self::Logs => (OTEL_BLRP_EXPORT_TIMEOUT, OTEL_BLRP_EXPORT_TIMEOUT_DEFAULT),
        }
    }

    /// What its batch processor reads itself; the metrics have none.
    fn batch_processor(self) -> Option<BatchProcessor> {
        match self {
            Self::Traces => Some(BatchProcessor {
                delay: OTEL_BSP_SCHEDULE_DELAY,
                queue: OTEL_BSP_MAX_QUEUE_SIZE,
                batch: OTEL_BSP_MAX_EXPORT_BATCH_SIZE,
                default_queue: OTEL_BSP_MAX_QUEUE_SIZE_DEFAULT,
                item: "span",
                queue_slot_bytes: slot_bytes::<SpanData>(),
                batch_slot_bytes: size_of::<SpanData>(),
            }),
            Self::Metrics => None,
            Self::Logs => Some(BatchProcessor {
                delay: OTEL_BLRP_SCHEDULE_DELAY,
                queue: OTEL_BLRP_MAX_QUEUE_SIZE,
                batch: OTEL_BLRP_MAX_EXPORT_BATCH_SIZE,
                default_queue: OTEL_BLRP_MAX_QUEUE_SIZE_DEFAULT,
                item: "log record",
                queue_slot_bytes: slot_bytes::<QueuedLogRecord>(),
                batch_slot_bytes: size_of::<QueuedLogRecord>(),
            }),
        }
    }

    /// Its own variable for `setting`, then the general one: the
    /// specification's `OTEL_EXPORTER_OTLP_{SIGNAL}_CERTIFICATE`,
    /// `_CLIENT_CERTIFICATE` and `_CLIENT_KEY`.
    fn tls_vars(self, setting: TlsSetting) -> [&'static str; 2] {
        use TlsSetting::{Authorities, Certificate, Key};
        let own = match (self, setting) {
            (Self::Traces, Authorities) => "OTEL_EXPORTER_OTLP_TRACES_CERTIFICATE",
            (Self::Traces, Certificate) => "OTEL_EXPORTER_OTLP_TRACES_CLIENT_CERTIFICATE",
            (Self::Traces, Key) => "OTEL_EXPORTER_OTLP_TRACES_CLIENT_KEY",
            (Self::Metrics, Authorities) => "OTEL_EXPORTER_OTLP_METRICS_CERTIFICATE",
            (Self::Metrics, Certificate) => "OTEL_EXPORTER_OTLP_METRICS_CLIENT_CERTIFICATE",
            (Self::Metrics, Key) => "OTEL_EXPORTER_OTLP_METRICS_CLIENT_KEY",
            (Self::Logs, Authorities) => "OTEL_EXPORTER_OTLP_LOGS_CERTIFICATE",
            (Self::Logs, Certificate) => "OTEL_EXPORTER_OTLP_LOGS_CLIENT_CERTIFICATE",
            (Self::Logs, Key) => "OTEL_EXPORTER_OTLP_LOGS_CLIENT_KEY",
        };
        let general = match setting {
            Authorities => OTEL_EXPORTER_OTLP_CERTIFICATE,
            Certificate => OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE,
            Key => OTEL_EXPORTER_OTLP_CLIENT_KEY,
        };
        [own, general]
    }
}

pub(crate) struct Exporters {
    pub tracer: opentelemetry_sdk::trace::Tracer,
    pub tracer_provider: SdkTracerProvider,
    /// `Some` only when the metrics export.
    pub meter_provider: Option<SdkMeterProvider>,
    /// `Some` only when the logs export.
    pub logger_provider: Option<SdkLoggerProvider>,
    /// What the console's line about the exporters says.
    pub summary: Summary,
}

/// What the subscriber, once installed, says about what was built.
pub(crate) struct Summary {
    /// The `service.name` the resource carries.
    pub service: String,
    /// Where each signal goes — traces, metrics, logs.
    pub destinations: [Destination; 3],
    /// The sampler, as the boot line says it.
    pub sampler: SamplerChoice,
    /// The variables passed over — a value the specification says to pass
    /// over, or a variable this build serves nowhere — said once a subscriber
    /// can hear it.
    pub passed_over: Vec<PassedOver>,
    /// Each signal exporting over plain HTTP off this machine, and where, shown
    /// without its credential.
    pub in_plaintext: Vec<(Signal, String)>,
}

/// Where one signal goes, as the boot line says it: an operator reads there
/// why nothing is exported.
#[derive(Debug)]
pub(crate) enum Destination {
    /// Exported to this endpoint, shown without its credential.
    Endpoint(String),
    /// Off: its selector says `none`.
    SwitchedOff(Signal),
    /// Off: `OTEL_SDK_DISABLED` is `true`.
    SdkDisabled,
    /// Off: nothing names an endpoint for it.
    NoEndpoint,
}

impl std::fmt::Display for Destination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Endpoint(shown) => f.write_str(shown),
            Self::SwitchedOff(signal) => write!(f, "off ({}=none)", signal.exporter_var()),
            Self::SdkDisabled => write!(f, "off ({OTEL_SDK_DISABLED}=true)"),
            Self::NoEndpoint => f.write_str("off (no endpoint)"),
        }
    }
}

/// A variable passed over, and why, as its line says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PassedOver {
    pub variable: &'static str,
    pub reason: &'static str,
}

/// What a variable read in milliseconds must hold, one read as a count, and
/// a batch processor's queue or batch size: each refusal adds its type's
/// bound, since a number past it is a whole number still.
const MILLISECONDS: &str = "must be a whole number of milliseconds";
const COUNT: &str = "must be a whole number";
const SIZE: &str = "must be a positive whole number";

/// The shortest schedule delay, in milliseconds: the SDK's worker waits for
/// each message no longer than the delay, so on a zero one it polls its
/// channel without pause.
const DELAY_FLOOR_MS: u64 = 1;

/// The largest queue a batch processor may hold: the SDK allocates every slot
/// of it when the processor starts, so a size past what the process holds
/// aborts the boot naming no variable.
const QUEUE_CEILING: usize = 1 << 20;

/// What the SDK's log processor queues and batches: each record boxed with
/// its scope.
type QueuedLogRecord = Box<(SdkLogRecord, InstrumentationScope)>;

/// The variables a batch processor reads itself — its delay, in
/// milliseconds, and its queue and batch sizes — the queue size it takes
/// when its variable is unset, and what one slot of its queue and of its
/// batch holds.
#[derive(Clone, Copy, Debug)]
struct BatchProcessor {
    delay: &'static str,
    queue: &'static str,
    batch: &'static str,
    default_queue: usize,
    /// What the queue holds, as a refusal names it.
    item: &'static str,
    /// The bytes one slot of the queue takes — the channel's stamp beside the
    /// item the SDK queues, a span whole, a log record boxed.
    queue_slot_bytes: usize,
    /// The bytes one item of the batch buffer takes, which the SDK's worker
    /// allocates as large as the batch when it starts.
    batch_slot_bytes: usize,
}

pub(crate) fn build(config: &OpenTelemetryConfig) -> Result<Exporters, OpenTelemetryError> {
    build_reading(config, AppIdentity::current(), nest_rs_config::env_var)
}

/// [`build`], with the binary's identity and the variables read through
/// `as_written`.
fn build_reading(
    config: &OpenTelemetryConfig,
    binary: Option<&AppIdentity>,
    as_written: impl Fn(&str) -> Option<String>,
) -> Result<Exporters, OpenTelemetryError> {
    let read = blank_as_unset(&as_written);
    refuse_config_file(&read)?;
    read_propagators(&read)?;
    let mut passed_over = Vec::new();
    let (sampler, sampler_choice) = sampler(config, &read, &mut passed_over)?;
    let span_limits = span_limits(&read)?;
    let resource = build_resource(config, binary, &as_written)?;
    let interval = metric_interval(config, &read)?;
    let disabled = sdk_disabled(&read, &mut passed_over);
    let [traces, metrics, logs] = Signal::ALL.map(|signal| route(signal, disabled, config, &read));
    let ((traces, to_traces), (metrics, to_metrics), (logs, to_logs)) = (traces?, metrics?, logs?);
    let temporality = match metrics {
        Some(_) => temporality(&read)?,
        None => None,
    };
    passed_over.extend(unserved(&read, logs.is_some()));
    let exports = [traces, metrics, logs];
    let in_plaintext = Signal::ALL
        .into_iter()
        .zip(&exports)
        .filter_map(|(signal, export)| {
            let export = export
                .as_ref()
                .filter(|export| export.plaintext && !export.loopback)?;
            Some((signal, endpoint_shown(export.url.as_str())))
        })
        .collect();
    let [traces_client, metrics_client, logs_client] = exporter_clients(&exports)?;
    let [traces, metrics, logs] = exports;

    global::set_text_map_propagator(global_propagator());
    let service = resource
        .get(&Key::new(SERVICE_NAME))
        .map(|name| name.to_string())
        .unwrap_or_default();

    let mut tracer_builder = tracer_provider(resource.clone(), sampler, span_limits);
    if let (Some(export), Some(client)) = (&traces, traces_client) {
        let span_exporter = export
            .configure(SpanExporter::builder().with_http().with_http_client(client))
            .build()
            .map_err(not_built(Signal::Traces, export))?;
        tracer_builder = tracer_builder.with_batch_exporter(span_exporter);
    }
    let tracer_provider = tracer_builder.build();
    let tracer = tracer_provider.tracer(service.clone());
    global::set_tracer_provider(tracer_provider.clone());

    let meter_provider = match (&metrics, metrics_client) {
        (Some(export), Some(client)) => {
            let builder = export.configure(
                MetricExporter::builder()
                    .with_http()
                    .with_http_client(client),
            );
            let metric_exporter = match temporality {
                Some(temporality) => builder.with_temporality(temporality),
                None => builder,
            }
            .build()
            .map_err(not_built(Signal::Metrics, export))?;
            let reader = PeriodicReader::builder(metric_exporter)
                .with_interval(interval)
                .build();
            let meter_provider = SdkMeterProvider::builder()
                .with_resource(resource.clone())
                .with_reader(reader)
                .build();
            global::set_meter_provider(meter_provider.clone());
            Some(meter_provider)
        }
        _ => None,
    };

    let logger_provider = match (&logs, logs_client) {
        (Some(export), Some(client)) => {
            let log_exporter = export
                .configure(LogExporter::builder().with_http().with_http_client(client))
                .build()
                .map_err(not_built(Signal::Logs, export))?;
            Some(
                SdkLoggerProvider::builder()
                    .with_resource(resource)
                    .with_batch_exporter(log_exporter)
                    .build(),
            )
        }
        _ => None,
    };

    Ok(Exporters {
        tracer,
        tracer_provider,
        meter_provider,
        logger_provider,
        summary: Summary {
            service,
            destinations: [to_traces, to_metrics, to_logs],
            sampler: sampler_choice,
            passed_over,
            in_plaintext,
        },
    })
}

/// The tracer provider every build starts from, an exporter or none added
/// after: the resource, the sampler and the span limits resolved.
fn tracer_provider(
    resource: Resource,
    sampler: Sampler,
    span_limits: SpanLimits,
) -> TracerProviderBuilder {
    SdkTracerProvider::builder()
        .with_resource(resource)
        // The SDK adopts the framework's ids rather than minting a second pair; see
        // `id_generator`.
        .with_id_generator(crate::id_generator::AdoptFrameworkIds::default())
        .with_sampler(sampler)
        .with_span_limits(span_limits)
}

/// What `OTEL_TRACES_SAMPLER` may name: the samplers the SDK builds.
const SAMPLERS: &str = "must name always_on, always_off, traceidratio, parentbased_always_on, \
                        parentbased_always_off or parentbased_traceidratio, the samplers the SDK \
                        builds, in lowercase";

/// What the boot line says of the sampler.
#[derive(Debug)]
pub(crate) struct SamplerChoice {
    /// Its name, as `OTEL_TRACES_SAMPLER` spells it.
    pub name: &'static str,
    /// What chose it: `OTEL_TRACES_SAMPLER`, or the ratio pinned in code.
    pub source: &'static str,
    /// The share of traces it samples, on a sampler reading a ratio.
    pub ratio: Option<f64>,
}

/// The sampler, and what the boot line says of it: the one `OTEL_TRACES_SAMPLER`
/// names, matched as the SDK matches it, a ratio's from `OTEL_TRACES_SAMPLER_ARG`
/// (1, the specification's default, when unset); else the ratio the app
/// pinned, parent-based. A pinned ratio outside `[0, 1]` is refused naming its
/// field, whichever applies.
///
/// What the SDK would pass over is refused here, naming the variable: a
/// sampler it does not build (`jaeger_remote`, `xray`, a misspelling) and a
/// ratio that does not read as one between 0 and 1, where it would sample every
/// trace over the pinned ratio. An argument beside a sampler that takes none,
/// or with no sampler, joins `passed_over`, as the specification orders.
fn sampler(
    config: &OpenTelemetryConfig,
    read: &impl Fn(&str) -> Option<String>,
    passed_over: &mut Vec<PassedOver>,
) -> Result<(Sampler, SamplerChoice), ConfigError> {
    let pinned = config.trace_sample_ratio;
    if !(0.0..=1.0).contains(&pinned) {
        return Err(ConfigError::parse(
            PINNED_RATIO,
            "must be a number between 0 and 1",
        ));
    }
    let argument = read(OTEL_TRACES_SAMPLER_ARG);
    let ratio = || -> Result<f64, ConfigError> {
        let Some(raw) = argument.as_deref() else {
            return Ok(1.0);
        };
        raw.parse::<f64>()
            .ok()
            .filter(|ratio| (0.0..=1.0).contains(ratio))
            .ok_or_else(|| {
                ConfigError::parse(
                    OTEL_TRACES_SAMPLER_ARG,
                    "must be a number between 0 and 1, the share of traces sampled",
                )
            })
    };
    let parent_based = |root: Sampler| Sampler::ParentBased(Box::new(root));
    let chosen = read(OTEL_TRACES_SAMPLER);
    let (sampler, name, ratio) = match chosen.as_deref() {
        None => (
            parent_based(Sampler::TraceIdRatioBased(pinned)),
            "parentbased_traceidratio",
            Some(pinned),
        ),
        Some("always_on") => (Sampler::AlwaysOn, "always_on", None),
        Some("always_off") => (Sampler::AlwaysOff, "always_off", None),
        Some("traceidratio") => {
            let ratio = ratio()?;
            (
                Sampler::TraceIdRatioBased(ratio),
                "traceidratio",
                Some(ratio),
            )
        }
        Some("parentbased_always_on") => (
            parent_based(Sampler::AlwaysOn),
            "parentbased_always_on",
            None,
        ),
        Some("parentbased_always_off") => (
            parent_based(Sampler::AlwaysOff),
            "parentbased_always_off",
            None,
        ),
        Some("parentbased_traceidratio") => {
            let ratio = ratio()?;
            (
                parent_based(Sampler::TraceIdRatioBased(ratio)),
                "parentbased_traceidratio",
                Some(ratio),
            )
        }
        Some(_) => return Err(ConfigError::parse(OTEL_TRACES_SAMPLER, SAMPLERS)),
    };
    if argument.is_some() && (chosen.is_none() || ratio.is_none()) {
        passed_over.push(PassedOver {
            variable: OTEL_TRACES_SAMPLER_ARG,
            reason: "OTEL_TRACES_SAMPLER names no ratio sampler, so nothing reads it",
        });
    }
    let source = if chosen.is_some() {
        OTEL_TRACES_SAMPLER
    } else {
        "trace_sample_ratio"
    };
    Ok((
        sampler,
        SamplerChoice {
            name,
            source,
            ratio,
        },
    ))
}

/// The metric exporter's temporality, `OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE`,
/// read by the SDK's own parser — `cumulative`, `delta` or `lowmemory`, in any
/// case — and handed to the exporter; anything else is refused naming the
/// variable, where the SDK would pass it over and export cumulative sums.
fn temporality(read: &impl Fn(&str) -> Option<String>) -> Result<Option<Temporality>, ConfigError> {
    let variable = OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE;
    let Some(raw) = read(variable) else {
        return Ok(None);
    };
    raw.parse::<Temporality>().map(Some).map_err(|()| {
        ConfigError::parse(
            variable,
            "must be cumulative, delta or lowmemory, in any case",
        )
    })
}

/// The metric export period: `OTEL_METRIC_EXPORT_INTERVAL`, in milliseconds,
/// over the one pinned, each held to a second to an hour; a value that is not
/// a whole number of milliseconds is refused, where the SDK would pass it
/// over.
fn metric_interval(
    config: &OpenTelemetryConfig,
    read: &impl Fn(&str) -> Option<String>,
) -> Result<Duration, ConfigError> {
    match milliseconds(OTEL_METRIC_EXPORT_INTERVAL, read)? {
        Some(interval) => bounded_interval(interval, OTEL_METRIC_EXPORT_INTERVAL),
        None => bounded_interval(config.metric_interval, PINNED_INTERVAL),
    }
}

/// The shortest metric export period: a periodic reader on a shorter one
/// exports in a tight loop.
const METRIC_INTERVAL_FLOOR: Duration = Duration::from_secs(1);
const METRIC_INTERVAL_FLOOR_WHY: &str = "a periodic reader on a shorter period exports in a \
                                         tight loop, which costs the collector more than any \
                                         metric is worth";
/// The longest: past it, a metric reaches the collector after what reads it.
const METRIC_INTERVAL_CEILING: Duration = Duration::from_secs(60 * 60);
const METRIC_INTERVAL_CEILING_WHY: &str = "a metric exported less often than hourly reaches the \
                                           collector after the dashboards and alerts that read \
                                           it have moved on, and the reader holds an hour of \
                                           state in memory meanwhile";

/// `interval`, set by `named`, refused outside a second to an hour.
fn bounded_interval(interval: Duration, named: &str) -> Result<Duration, ConfigError> {
    if interval < METRIC_INTERVAL_FLOOR {
        return Err(ConfigError::parse(
            named,
            format!(
                "is {interval:?}, below the {METRIC_INTERVAL_FLOOR:?} it must be at least — \
                 {METRIC_INTERVAL_FLOOR_WHY}"
            ),
        ));
    }
    if interval > METRIC_INTERVAL_CEILING {
        return Err(ConfigError::parse(
            named,
            format!(
                "is {interval:?}, above the {METRIC_INTERVAL_CEILING:?} it must be at most — \
                 {METRIC_INTERVAL_CEILING_WHY}"
            ),
        ));
    }
    Ok(interval)
}

/// Whether `OTEL_SDK_DISABLED` turns every signal off: `true`, in any case,
/// does. Anything else leaves the SDK on, as the specification orders, and a
/// value that is not `false` either joins `passed_over`.
fn sdk_disabled(read: &impl Fn(&str) -> Option<String>, passed_over: &mut Vec<PassedOver>) -> bool {
    match SdkSwitch::read(read) {
        SdkSwitch::Off => true,
        SdkSwitch::On => false,
        SdkSwitch::Unrecognised => {
            passed_over.push(PassedOver {
                variable: OTEL_SDK_DISABLED,
                reason: "neither true nor false, so read as false, as the specification orders",
            });
            false
        }
    }
}

/// Where `signal` goes: off under `OTEL_SDK_DISABLED` or its selector's
/// `none`, else exported where [`export`] finds an endpoint. `otlp` named with
/// no endpoint for the signal is refused, naming where one is set; unset, the
/// signal stays quiet.
fn route(
    signal: Signal,
    disabled: bool,
    config: &OpenTelemetryConfig,
    read: &impl Fn(&str) -> Option<String>,
) -> Result<(Option<Export>, Destination), ConfigError> {
    let selection = signal.selection(read)?;
    if disabled {
        return Ok((None, Destination::SdkDisabled));
    }
    if selection == Selection::None {
        return Ok((None, Destination::SwitchedOff(signal)));
    }
    match export(signal, config, read)? {
        Some(export) => {
            let shown = endpoint_shown(export.url.as_str());
            Ok((Some(export), Destination::Endpoint(shown)))
        }
        None if selection == Selection::Otlp => Err(ConfigError::parse(
            signal.exporter_var(),
            format!(
                "asks for an OTLP export, but nothing sets an endpoint for {} — set {} or {} \
                 (or {PINNED_ENDPOINT}), or turn the signal off with none",
                signal.name(),
                signal.endpoint_var(),
                OTEL_EXPORTER_OTLP_ENDPOINT,
            ),
        )),
        None => Ok((None, Destination::NoEndpoint)),
    }
}

/// Where one signal exports, and what it exports through.
struct Export {
    url: url::Url,
    /// The variable, or the field in code, that set the endpoint.
    set_by: &'static str,
    plaintext: bool,
    /// On this host's loopback, dialled directly: a proxy the environment
    /// names would reach its own host's loopback, and carry a header sent in
    /// clear there off the machine.
    loopback: bool,
    timeout: Duration,
    compression: Option<Compression>,
    /// The headers every export carries, as the specification resolves them:
    /// a value may be a collector's credential. Each value is kept as written,
    /// its escapes undecoded, since `with_headers` decodes what it is handed:
    /// a value decoded here too would be decoded twice.
    headers: HashMap<String, Secret<String>>,
    tls: ClientTls,
    names: SignalTls,
}

impl Export {
    /// `builder` pointed at this export's endpoint, bounded by its timeout,
    /// compressed as resolved and carrying its headers: each setting handed
    /// over, so what the SDK reads again of them its own way changes nothing.
    fn configure<B: WithExportConfig + WithHttpConfig>(&self, builder: B) -> B {
        let builder = builder
            .with_endpoint(self.url.as_str())
            .with_protocol(Protocol::HttpBinary)
            .with_timeout(self.timeout)
            .with_headers(
                self.headers
                    .iter()
                    .map(|(name, value)| (name.clone(), value.expose_secret().clone()))
                    .collect(),
            );
        match self.compression {
            Some(compression) => builder.with_compression(compression),
            None => builder,
        }
    }
}

/// Where `signal` exports — its own endpoint variable, used as given; then
/// `OTEL_EXPORTER_OTLP_ENDPOINT`, then the pinned endpoint, each a base the
/// signal's path is set on — or `None` when nothing sets one. A protocol
/// variable naming another than http/protobuf is refused rather than
/// overridden: the collector's port speaks what it names. So are userinfo in
/// any endpoint, a query on a base, a compression this exporter does not
/// build, a number its batch processor reads that does not parse, a header the
/// SDK would drop, and a credential beside or in a plaintext endpoint on
/// another host.
fn export(
    signal: Signal,
    config: &OpenTelemetryConfig,
    read: &impl Fn(&str) -> Option<String>,
) -> Result<Option<Export>, ConfigError> {
    let (url, set_by) = if let Some(full) = read(signal.endpoint_var()) {
        let set_by = signal.endpoint_var();
        (parsed_endpoint(&full, set_by)?, set_by)
    } else if let Some(base) = read(OTEL_EXPORTER_OTLP_ENDPOINT) {
        (
            base_endpoint(&base, signal, OTEL_EXPORTER_OTLP_ENDPOINT)?,
            OTEL_EXPORTER_OTLP_ENDPOINT,
        )
    } else if let Some(base) = config.otlp_endpoint.as_deref() {
        (
            base_endpoint(base, signal, PINNED_ENDPOINT)?,
            PINNED_ENDPOINT,
        )
    } else {
        return Ok(None);
    };
    let plaintext = match url.scheme() {
        "https" => false,
        "http" => true,
        _ => {
            return Err(ConfigError::parse(
                set_by,
                "must be an http:// or https:// URL — OTLP/HTTP is spoken over one of the two",
            ));
        }
    };
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ConfigError::parse(
            set_by,
            format!(
                "carries userinfo, which no export sends — the HTTP client builds each request \
                 without it, so a collector's credential there never reaches the collector; \
                 set it as a header in {OTEL_EXPORTER_OTLP_HEADERS} (or {}) instead",
                signal.headers_var(),
            ),
        ));
    }
    for variable in [signal.protocol_var(), OTEL_EXPORTER_OTLP_PROTOCOL] {
        if let Some(protocol) = read(variable) {
            if protocol.trim() != OTEL_EXPORTER_OTLP_PROTOCOL_HTTP_PROTOBUF {
                return Err(ConfigError::parse(
                    variable,
                    "must be http/protobuf, the one OTLP protocol this exporter speaks",
                ));
            }
            break;
        }
    }
    let compression = compression(signal, read)?;
    refuse_unservable_batch_numbers(signal, read)?;
    let (headers, headers_set_by) = match headers(signal, read)? {
        Some(headers) => (headers.pairs, Some(headers.set_by)),
        None => (HashMap::new(), None),
    };
    let (tls, names) = signal_tls(signal, &config.tls, read)?;
    let loopback = url.host_str().is_some_and(is_loopback);
    if plaintext {
        tls.refuse_beside_plaintext(&names, set_by)?;
        if !loopback {
            refuse_credentials_in_clear(&url, set_by, headers_set_by)?;
        }
    }
    let timeout = export_bound(
        export_timeout(signal.timeout_var(), read)?,
        processor_timeout(signal, read)?,
    )?;
    Ok(Some(Export {
        url,
        set_by,
        plaintext,
        loopback,
        timeout,
        compression,
        headers,
        tls,
        names,
    }))
}

/// `raw`, set by `set_by`, trimmed and parsed as a URL, its refusal naming the
/// setter and never quoting the value, which may carry a credential.
fn parsed_endpoint(raw: &str, set_by: &str) -> Result<url::Url, ConfigError> {
    url::Url::parse(raw.trim())
        .map_err(|error| ConfigError::parse(set_by, format!("does not parse as a URL: {error}")))
}

/// `base`, set by `set_by`, with `signal`'s path set on its own, one slash
/// between. The specification sends each signal to a path relative to the
/// base and lets an SDK change the URL no other way, which leaves a query no
/// place: one is refused, where appending the path as a string would hide it
/// inside the query and export to the collector's root.
fn base_endpoint(base: &str, signal: Signal, set_by: &str) -> Result<url::Url, ConfigError> {
    let mut url = parsed_endpoint(base, set_by)?;
    if url.query().is_some_and(|query| !query.is_empty()) {
        return Err(ConfigError::parse(
            set_by,
            format!(
                "carries a query, which no signal's path can follow — each signal is sent to a \
                 path relative to this base — so set each signal's own endpoint, used as given \
                 with its query ({}, {}, {}), or move the parameter to a header in \
                 {OTEL_EXPORTER_OTLP_HEADERS}",
                Signal::Traces.endpoint_var(),
                Signal::Metrics.endpoint_var(),
                Signal::Logs.endpoint_var(),
            ),
        ));
    }
    let path = format!("{}{}", url.path().trim_end_matches('/'), signal.path());
    url.set_path(&path);
    url.set_query(None);
    Ok(url)
}

/// How `signal`'s exports are compressed: its own variable, then
/// `OTEL_EXPORTER_OTLP_COMPRESSION` — `gzip`, the one compression the OTLP
/// specification names, or `none`, in any case. Anything else is refused
/// naming the variable rather than passed over, as the SDK would pass it,
/// while the deployment believes its exports compressed.
///
/// Matched untrimmed, as the SDK matches it: a value the two read apart would
/// let the SDK compress what this resolution left plain.
fn compression(
    signal: Signal,
    read: &impl Fn(&str) -> Option<String>,
) -> Result<Option<Compression>, ConfigError> {
    for variable in [signal.compression_var(), OTEL_EXPORTER_OTLP_COMPRESSION] {
        let Some(raw) = read(variable) else {
            continue;
        };
        return if raw.eq_ignore_ascii_case("gzip") {
            Ok(Some(Compression::Gzip))
        } else if raw.eq_ignore_ascii_case("none") {
            Ok(None)
        } else {
            Err(ConfigError::parse(
                variable,
                "must be gzip, the one compression the OTLP specification names and the one \
                 this exporter builds, or none",
            ))
        };
    }
    Ok(None)
}

/// Refuses a number `signal`'s batch processor reads itself that it cannot
/// serve as written, naming the variable and never quoting it: one that does
/// not parse, which it would replace by its default in silence — parsed as the
/// SDK parses it, untrimmed, so the two never read one apart; a queue or batch
/// size of zero, which the specification calls invalid and with which a queue
/// takes no span or record, every one dropped; a batch written above its
/// queue, which the specification forbids and the SDK lowers to the queue in
/// silence; and, as the metric period is bounded, what the SDK would turn
/// into a spin or into an allocation no process holds whatever the
/// specification's type admits — a delay under [`DELAY_FLOOR_MS`], on which
/// its worker polls without pause, and a queue above [`QUEUE_CEILING`], every
/// slot of which it allocates when the processor starts, beside a batch buffer
/// that may reach the queue.
fn refuse_unservable_batch_numbers(
    signal: Signal,
    read: &impl Fn(&str) -> Option<String>,
) -> Result<(), ConfigError> {
    let Some(processor) = signal.batch_processor() else {
        return Ok(());
    };
    let as_the_sdk_reads =
        |expected: String| format!("{expected}, its digits alone, as the SDK reads it");
    let delay = parsed(
        processor.delay,
        read,
        |raw| raw.parse::<u64>().ok(),
        || as_the_sdk_reads(format!("{MILLISECONDS} up to {}", u64::MAX)),
    )?;
    if delay.is_some_and(|delay| delay < DELAY_FLOOR_MS) {
        return Err(ConfigError::parse(
            processor.delay,
            format!(
                "must be at least {DELAY_FLOOR_MS} ms — the SDK's worker waits for each message \
                 no longer than the delay, so on a zero one it polls its channel without pause: \
                 a core for this processor, idle or not"
            ),
        ));
    }
    let size = |variable: &'static str| {
        parsed(
            variable,
            read,
            |raw| raw.parse::<usize>().ok().filter(|size| *size > 0),
            || as_the_sdk_reads(format!("{SIZE} up to {}", usize::MAX)),
        )
    };
    let queue = size(processor.queue)?.unwrap_or(processor.default_queue);
    if queue > QUEUE_CEILING {
        return Err(ConfigError::parse(
            processor.queue,
            format!(
                "must be at most {QUEUE_CEILING} — the SDK allocates the whole queue when the \
                 processor starts, {} bytes a {item} slot, and its worker a buffer as large as \
                 the batch, which may reach the queue, {} bytes a {item}: up to {} MiB at that \
                 ceiling, and a queue past what the process holds would abort the boot naming \
                 nothing",
                processor.queue_slot_bytes,
                processor.batch_slot_bytes,
                (QUEUE_CEILING * (processor.queue_slot_bytes + processor.batch_slot_bytes))
                    .div_ceil(1 << 20),
                item = processor.item,
            ),
        ));
    }
    if size(processor.batch)?.is_some_and(|batch| batch > queue) {
        return Err(ConfigError::parse(
            processor.batch,
            format!(
                "must be at most {} ({} when unset), as the specification orders — the \
                 processor exports no batch larger than its queue holds, and the SDK would \
                 lower it to the queue in silence",
                processor.queue, processor.default_queue,
            ),
        ));
    }
    Ok(())
}

/// The bytes one slot of a `sync_channel` of `T` takes, the channel the SDK
/// queues each processor's items on: std's bounded channel allocates every
/// slot up front, each a stamp beside the item.
const fn slot_bytes<T>() -> usize {
    size_of::<(usize, T)>()
}

/// The headers one signal's exports carry, and the variable that set them.
#[derive(Debug, PartialEq, Eq)]
struct Headers {
    set_by: &'static str,
    pairs: HashMap<String, Secret<String>>,
}

/// What a headers variable must hold.
const HEADERS: &str = "must list key=value pairs separated by commas, each key a header name and \
                       each value a header value, not empty, percent-encoded where it needs to \
                       be, as the SDK reads them";

/// The headers `signal`'s exports carry, and the variable that set them: its
/// own variable, then `OTEL_EXPORTER_OTLP_HEADERS`. A blank one is unset, as
/// the specification orders, so the general headers reach a signal whose own
/// variable is empty, which opentelemetry-otlp 0.33 alone reads as no header;
/// handed to the exporter, they are what it sends.
///
/// Parsed as the SDK parses them, so the two never read a pair apart; a pair
/// it would drop in silence — no `=`, an empty key or value, a key no header
/// may carry, a value no header may hold once decoded — is refused naming the
/// variable, never quoted. Each value is kept as written, trimmed where it
/// decodes: the exporter it is handed to decodes it, once.
fn headers(
    signal: Signal,
    read: &impl Fn(&str) -> Option<String>,
) -> Result<Option<Headers>, ConfigError> {
    let Some((variable, raw)) = [signal.headers_var(), OTEL_EXPORTER_OTLP_HEADERS]
        .into_iter()
        .find_map(|variable| read(variable).map(|raw| (variable, raw)))
    else {
        return Ok(None);
    };
    let mut pairs = HashMap::new();
    for member in raw
        .split_terminator(',')
        .map(str::trim)
        .filter(|member| !member.is_empty())
    {
        let pair = member.split_once('=').and_then(|(key, value)| {
            let key = key.trim();
            let (written, decoded) = match percent_decoded(value.trim()) {
                Some(decoded) => (value.trim(), decoded),
                None => (value, value.to_owned()),
            };
            let header = !key.is_empty()
                && !decoded.is_empty()
                && HeaderName::from_str(key).is_ok()
                && HeaderValue::from_str(&decoded).is_ok();
            header.then(|| (key.to_owned(), written.to_owned()))
        });
        let Some((key, value)) = pair else {
            return Err(ConfigError::parse(variable, HEADERS));
        };
        pairs.insert(key, Secret::new(value));
    }
    Ok((!pairs.is_empty()).then_some(Headers {
        set_by: variable,
        pairs,
    }))
}

/// Refuses what may carry a collector's credential to `url`, an `http://`
/// endpoint on another host, where RFC 6750 §5.3 sends one over TLS alone:
/// the headers the exports carry, set by `headers_set_by`, and the endpoint's
/// own query, which every request line carries. No header's or parameter's
/// name tells a credential apart, so each is held to it. Named by its variable
/// and the endpoint's setter, never quoted.
fn refuse_credentials_in_clear(
    url: &url::Url,
    set_by: &str,
    headers_set_by: Option<&'static str>,
) -> Result<(), ConfigError> {
    const REMEDY: &str = "export over https://, or to a collector on this host's loopback \
                          (localhost, 127.0.0.1, [::1])";
    if let Some(variable) = headers_set_by {
        return Err(ConfigError::parse(
            variable,
            format!(
                "is set beside {set_by}, which declares the export in plaintext to another \
                 host, where a collector's credential would cross the network in clear — \
                 {REMEDY}"
            ),
        ));
    }
    if url.query().is_some_and(|query| !query.is_empty()) {
        return Err(ConfigError::parse(
            set_by,
            format!(
                "declares the export in plaintext to another host and carries a query, where a \
                 collector's credential would cross the network in clear — {REMEDY}"
            ),
        ));
    }
    Ok(())
}

/// One bound on an export: the variable that set it, and its length, zero
/// where the variable holds no limit.
type Bound = (&'static str, Duration);

/// How long `signal`'s processor lets one export take: its variable —
/// `OTEL_BSP_EXPORT_TIMEOUT`, `OTEL_METRIC_EXPORT_TIMEOUT` or
/// `OTEL_BLRP_EXPORT_TIMEOUT` — in milliseconds, else the specification's
/// 30 s. The exporter's own timeout bounds the same export, so the earlier of
/// the two cuts it ([`export_bound`]); the batch processors this crate builds
/// read neither of theirs, so it is applied here.
fn processor_timeout(
    signal: Signal,
    read: &impl Fn(&str) -> Option<String>,
) -> Result<Bound, ConfigError> {
    let (variable, default) = signal.processor_timeout();
    Ok((variable, milliseconds(variable, read)?.unwrap_or(default)))
}

/// How long one export of a signal may take: its `signal` variable, then
/// `OTEL_EXPORTER_OTLP_TIMEOUT`, in milliseconds, then the SDK's 10 s.
///
/// That is the SDK's own resolution, which bounds its retries, so an attempt is
/// cut where the SDK stops waiting for it — but for a zero, the
/// specification's no limit, which [`export_bound`] reads; a value that is not
/// a whole number of milliseconds is refused, where the SDK would pass it over.
fn export_timeout(
    signal: &'static str,
    read: &impl Fn(&str) -> Option<String>,
) -> Result<Bound, ConfigError> {
    for variable in [signal, OTEL_EXPORTER_OTLP_TIMEOUT] {
        if let Some(timeout) = milliseconds(variable, read)? {
            return Ok((variable, timeout));
        }
    }
    Ok((
        OTEL_EXPORTER_OTLP_TIMEOUT,
        OTEL_EXPORTER_OTLP_TIMEOUT_DEFAULT,
    ))
}

/// What cuts one export: the earlier of the exporter's bound and its
/// processor's, a zero being no limit, as the specification reads a timeout's
/// zero — a zero handed to the client would cut every export before it is
/// sent. Both at zero leave nothing bounding the export, and every wait the
/// framework owns is bounded: refused naming both.
fn export_bound(
    (exporter, exporter_length): Bound,
    (processor, processor_length): Bound,
) -> Result<Duration, ConfigError> {
    [exporter_length, processor_length]
        .into_iter()
        .filter(|length| !length.is_zero())
        .min()
        .ok_or_else(|| {
            ConfigError::parse(
                exporter,
                format!(
                    "reads as no limit, as the specification reads a zero, and so does \
                     {processor} — nothing would bound an export, and every wait this framework \
                     owns is bounded: set either to a positive number of milliseconds"
                ),
            )
        })
}

/// `variable`, a whole number of milliseconds, or `None` when unset; any other
/// value is refused naming it, never passed over while the deployment
/// believes it applies.
fn milliseconds(
    variable: &'static str,
    read: &impl Fn(&str) -> Option<String>,
) -> Result<Option<Duration>, ConfigError> {
    parsed(
        variable,
        read,
        |raw| raw.trim().parse().ok().map(Duration::from_millis),
        || format!("{MILLISECONDS} up to {}", u64::MAX),
    )
}

/// `variable` as `parse` reads it, or `None` when unset; a value `parse`
/// reads as nothing is refused naming the variable and what it must hold,
/// never quoted.
fn parsed<T>(
    variable: &'static str,
    read: &impl Fn(&str) -> Option<String>,
    parse: impl FnOnce(&str) -> Option<T>,
    must: impl FnOnce() -> String,
) -> Result<Option<T>, ConfigError> {
    read(variable)
        .map(|raw| parse(&raw).ok_or_else(|| ConfigError::parse(variable, must())))
        .transpose()
}

/// The span limits, each from its own variable over the SDK's default: a
/// span's, an event's and a link's attribute count falling back on
/// `OTEL_ATTRIBUTE_COUNT_LIMIT`, as the specification orders a model's limit
/// before the general one. Handed to the tracer, so the SDK — which reads
/// three of them and none of the attribute counts but a span's — reads none
/// again; a value that is not a whole number a `u32` holds, the SDK's type,
/// is refused naming its variable.
fn span_limits(read: &impl Fn(&str) -> Option<String>) -> Result<SpanLimits, ConfigError> {
    let count = |variable: &'static str| {
        parsed(
            variable,
            read,
            |raw| raw.trim().parse::<u32>().ok(),
            || format!("{COUNT} up to {}", u32::MAX),
        )
    };
    let general = count(OTEL_ATTRIBUTE_COUNT_LIMIT)?;
    let defaults = SpanLimits::default();
    Ok(SpanLimits {
        max_attributes_per_span: count(OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT)?
            .or(general)
            .unwrap_or(defaults.max_attributes_per_span),
        max_events_per_span: count(OTEL_SPAN_EVENT_COUNT_LIMIT)?
            .unwrap_or(defaults.max_events_per_span),
        max_links_per_span: count(OTEL_SPAN_LINK_COUNT_LIMIT)?
            .unwrap_or(defaults.max_links_per_span),
        max_attributes_per_event: count(OTEL_EVENT_ATTRIBUTE_COUNT_LIMIT)?
            .or(general)
            .unwrap_or(defaults.max_attributes_per_event),
        max_attributes_per_link: count(OTEL_LINK_ATTRIBUTE_COUNT_LIMIT)?
            .or(general)
            .unwrap_or(defaults.max_attributes_per_link),
    })
}

/// A specification variable this build serves nowhere, the value that names
/// what it does instead, and why it is passed over.
struct Unserved {
    variable: &'static str,
    /// The value, in any case, that asks for what the build does, which is
    /// honoured in silence.
    honoured: Option<&'static str>,
    reason: &'static str,
}

/// Why a value-length limit is passed over.
const NO_VALUE_LENGTH_LIMIT: &str =
    "this SDK limits no attribute value's length, so each is exported whole";

/// Why the transport-security flag is passed over, the value `false` asking for
/// nothing this exporter does otherwise.
const GRPC_ALONE: &str = "it applies to gRPC alone; over OTLP/HTTP the endpoint's scheme decides";

/// Every specification variable no crate this build links reads, nor offers a
/// setting for (opentelemetry_sdk 0.33, opentelemetry-otlp 0.33), and the
/// SDK's own the processor this crate builds ignores.
const UNSERVED: &[Unserved] = &[
    Unserved {
        variable: OTEL_ATTRIBUTE_VALUE_LENGTH_LIMIT,
        honoured: None,
        reason: NO_VALUE_LENGTH_LIMIT,
    },
    Unserved {
        variable: OTEL_SPAN_ATTRIBUTE_VALUE_LENGTH_LIMIT,
        honoured: None,
        reason: NO_VALUE_LENGTH_LIMIT,
    },
    Unserved {
        variable: OTEL_LOGRECORD_ATTRIBUTE_VALUE_LENGTH_LIMIT,
        honoured: None,
        reason: NO_VALUE_LENGTH_LIMIT,
    },
    Unserved {
        variable: OTEL_LOGRECORD_ATTRIBUTE_COUNT_LIMIT,
        honoured: None,
        reason: "this SDK limits no log record's attributes",
    },
    Unserved {
        variable: OTEL_METRICS_EXEMPLAR_FILTER,
        honoured: Some("always_off"),
        reason: "this SDK attaches no exemplar to a metric, which always_off asks for",
    },
    Unserved {
        variable: OTEL_EXPORTER_OTLP_METRICS_DEFAULT_HISTOGRAM_AGGREGATION,
        honoured: Some("explicit_bucket_histogram"),
        reason: "this exporter aggregates a histogram in explicit buckets alone, which \
                 explicit_bucket_histogram asks for",
    },
    Unserved {
        variable: OTEL_LOG_LEVEL,
        honoured: None,
        reason: "the SDK's own lines follow the console's log filter, as every library's do",
    },
    Unserved {
        variable: OTEL_BSP_MAX_CONCURRENT_EXPORTS,
        honoured: Some("1"),
        reason: "the batch span processor this build runs exports one batch at a time",
    },
    Unserved {
        variable: OTEL_EXPORTER_OTLP_INSECURE,
        honoured: Some("false"),
        reason: GRPC_ALONE,
    },
    Unserved {
        variable: OTEL_EXPORTER_OTLP_TRACES_INSECURE,
        honoured: Some("false"),
        reason: GRPC_ALONE,
    },
    Unserved {
        variable: OTEL_EXPORTER_OTLP_METRICS_INSECURE,
        honoured: Some("false"),
        reason: GRPC_ALONE,
    },
    Unserved {
        variable: OTEL_EXPORTER_OTLP_LOGS_INSECURE,
        honoured: Some("false"),
        reason: GRPC_ALONE,
    },
];

/// The variables `read` sets that this build serves nowhere, each with why,
/// but for a value naming what the build does: a line on correct
/// configuration would teach an operator to filter the target. With the logs
/// exporting, `OTEL_ATTRIBUTE_COUNT_LIMIT` is one too where no log record's
/// own limit is set: it bounds a span's attributes, and no record's.
fn unserved(read: &impl Fn(&str) -> Option<String>, logs_export: bool) -> Vec<PassedOver> {
    let mut said: Vec<PassedOver> = UNSERVED
        .iter()
        .filter(|unserved| {
            read(unserved.variable).is_some_and(|value| {
                unserved
                    .honoured
                    .is_none_or(|honoured| !value.trim().eq_ignore_ascii_case(honoured))
            })
        })
        .map(|unserved| PassedOver {
            variable: unserved.variable,
            reason: unserved.reason,
        })
        .collect();
    if logs_export
        && read(OTEL_ATTRIBUTE_COUNT_LIMIT).is_some()
        && read(OTEL_LOGRECORD_ATTRIBUTE_COUNT_LIMIT).is_none()
    {
        said.push(PassedOver {
            variable: OTEL_ATTRIBUTE_COUNT_LIMIT,
            reason: "it bounds a span's, an event's and a link's attributes; this SDK limits no \
                     log record's",
        });
    }
    said
}

/// Which variable set each piece of one signal's TLS material, as its refusals
/// name it.
#[derive(Clone, Copy, Debug)]
struct SignalTls {
    signal: Signal,
    /// Per piece, the variable read, or `None` for the pinned one.
    authorities: Option<&'static str>,
    certificate: Option<&'static str>,
    key: Option<&'static str>,
}

impl TlsNames for SignalTls {
    fn named(&self, setting: TlsSetting, _material: &Material) -> String {
        match setting {
            TlsSetting::Authorities => self.authorities,
            TlsSetting::Certificate => self.certificate,
            TlsSetting::Key => self.key,
        }
        .unwrap_or(PINNED_TLS)
        .to_owned()
    }

    fn spellings(&self, setting: TlsSetting) -> String {
        let [own, general] = self.signal.tls_vars(setting);
        format!("{own} (or {general})")
    }
}

/// `signal`'s TLS material: each piece from the file its own variable names,
/// then the general one's, over `pinned`. A certificate and its key move as
/// one: half a pair is refused naming what was set and what is missing.
fn signal_tls(
    signal: Signal,
    pinned: &ClientTls,
    read: &impl Fn(&str) -> Option<String>,
) -> Result<(ClientTls, SignalTls), ConfigError> {
    let file = |setting: TlsSetting| -> Result<Option<(Material, &'static str)>, ConfigError> {
        let Some((path, var)) = signal
            .tls_vars(setting)
            .into_iter()
            .find_map(|var| read(var).map(|path| (path, var)))
        else {
            return Ok(None);
        };
        Material::from_file(std::path::Path::new(path.trim()))
            .map(|material| Some((material, var)))
            .map_err(|source| ConfigError::File {
                var: var.to_owned(),
                source,
            })
    };
    let authorities = file(TlsSetting::Authorities)?;
    let (cert, key) = (file(TlsSetting::Certificate)?, file(TlsSetting::Key)?);
    let mut names = SignalTls {
        signal,
        authorities: authorities.as_ref().map(|(_, var)| *var),
        certificate: None,
        key: None,
    };
    let identity = match (cert, key) {
        (Some((cert, cert_var)), Some((key, key_var))) => {
            names.certificate = Some(cert_var);
            names.key = Some(key_var);
            Some(TlsIdentity::new(cert, key))
        }
        (None, None) => None,
        (Some((_, given)), None) => {
            return Err(half_a_pair(given, &names.spellings(TlsSetting::Key)));
        }
        (None, Some((_, given))) => {
            return Err(half_a_pair(
                given,
                &names.spellings(TlsSetting::Certificate),
            ));
        }
    };
    let tls = pinned
        .clone()
        .overlaid(authorities.map(|(material, _)| material), identity);
    Ok((tls, names))
}

fn half_a_pair(given: &str, missing: &str) -> ConfigError {
    ConfigError::parse(
        given,
        format!("is set without {missing} — a client certificate is presented with its key"),
    )
}

/// How long reaching the collector (DNS, TCP and the TLS handshake) may take
/// before an export gives up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// The exporters' HTTP clients, one per signal that exports: TLS from its
/// material, no redirect followed, the connection bounded and each export by
/// its signal's [`export_timeout`], `https` alone unless the endpoint
/// declares plaintext with `http://` — the OTLP specification lets the
/// endpoint's scheme decide, and a collector beside the app is the common case
/// — and a loopback collector dialled directly, past any proxy the environment
/// names (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`), which reqwest otherwise
/// takes for every host, loopback included.
///
/// Built on a thread of their own, as the SDK builds its own: a blocking client
/// built on a runtime's thread panics.
fn exporter_clients(
    exports: &[Option<Export>; 3],
) -> Result<[Option<reqwest::blocking::Client>; 3], OpenTelemetryError> {
    struct Plan {
        plaintext: bool,
        loopback: bool,
        timeout: Duration,
        roots: Vec<reqwest::Certificate>,
        identity: Option<reqwest::Identity>,
    }
    let mut plans: [Option<Plan>; 3] = [None, None, None];
    for (plan, export) in plans.iter_mut().zip(exports) {
        if let Some(export) = export {
            let nest_rs_config::__private::ReqwestTls { roots, identity } =
                nest_rs_config::__private::reqwest_tls(&export.tls, &export.names)?;
            *plan = Some(Plan {
                plaintext: export.plaintext,
                loopback: export.loopback,
                timeout: export.timeout,
                roots,
                identity,
            });
        }
    }
    if plans.iter().all(Option::is_none) {
        return Ok([None, None, None]);
    }
    // reqwest builds from the process's provider, which rustls picks on its own
    // only when exactly one is compiled in.
    nest_rs_config::__private::crypto_provider();
    #[expect(
        clippy::map_err_ignore,
        reason = "a panic's payload may quote anything; the sentence says the build panicked"
    )]
    let built = std::thread::Builder::new()
        .name("otel-http-client".into())
        .spawn(move || {
            let client = |plan: Plan| {
                let mut builder = reqwest::blocking::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(CONNECT_TIMEOUT)
                    .timeout(plan.timeout)
                    .https_only(!plan.plaintext)
                    .tls_certs_only(plan.roots);
                if let Some(identity) = plan.identity {
                    builder = builder.identity(identity);
                }
                if plan.loopback {
                    builder = builder.no_proxy();
                }
                builder.build()
            };
            let [traces, metrics, logs] = plans;
            Ok::<_, reqwest::Error>([
                traces.map(client).transpose()?,
                metrics.map(client).transpose()?,
                logs.map(client).transpose()?,
            ])
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
            "the OTLP HTTP clients could not be built: {}",
            nest_rs_core::error_message(&error.without_url())
        ))
    })
}

/// An exporter the SDK would not build, said without its own error, whose
/// sentence quotes the endpoint whole.
fn not_built(
    signal: Signal,
    export: &Export,
) -> impl FnOnce(opentelemetry_otlp::ExporterBuildError) -> OpenTelemetryError + '_ {
    move |_| {
        OpenTelemetryError::Otlp(format!(
            "the OTLP {} exporter for {}, set by {}, could not be built",
            signal.name(),
            endpoint_shown(export.url.as_str()),
            export.set_by,
        ))
    }
}

/// The resource every signal carries: what the app pinned, and the binary's
/// name and version, under `OTEL_RESOURCE_ATTRIBUTES`, under
/// `OTEL_SERVICE_NAME`, which wins. `read` is the environment as written, a
/// blank value kept: a blank `OTEL_SERVICE_NAME` is refused, never read as
/// unset while the SDK's detector reads it as the name.
fn build_resource(
    config: &OpenTelemetryConfig,
    binary: Option<&AppIdentity>,
    read: &impl Fn(&str) -> Option<String>,
) -> Result<Resource, ConfigError> {
    let pinned = config.pinned_service_name()?;
    let attributes = resource_attributes(read)?;
    let service_name = service_name(read)?;
    // The SDK's environment detector, which `Resource::builder` runs, reads
    // `OTEL_RESOURCE_ATTRIBUTES` undecoded: the attributes are read above.
    let detected = Resource::builder_empty()
        .with_detectors(&[
            Box::new(SdkProvidedResourceDetector),
            Box::new(TelemetryResourceDetector),
        ])
        .build();
    let mut ours = vec![KeyValue::new(
        SERVICE_INSTANCE_ID,
        config
            .service_instance_id
            .clone()
            .unwrap_or_else(|| Uuid::now_v7().to_string()),
    )];
    let name = pinned
        .map(str::to_owned)
        .or_else(|| binary.map(|binary| binary.name.clone()));
    let version = config
        .service_version
        .clone()
        .or_else(|| binary.map(|binary| binary.version.clone()));
    let named = name.is_some();
    ours.extend(name.map(|name| KeyValue::new(SERVICE_NAME, name)));
    ours.extend(version.map(|version| KeyValue::new(SERVICE_VERSION, version)));
    ours.extend(
        config
            .deployment_environment
            .clone()
            .map(|environment| KeyValue::new(DEPLOYMENT_ENVIRONMENT_NAME, environment)),
    );
    // The SDK names a service no variable named `unknown_service`, or
    // `unknown_service:` and the executable's name (the resource SDK
    // specification): a fallback, which a name the app or its binary gives
    // replaces.
    let fallback = |key: &Key, value: &opentelemetry::Value| {
        key.as_str() == SERVICE_NAME && value.as_str().starts_with("unknown_service")
    };
    let over: Vec<KeyValue> = detected
        .iter()
        .filter(|(key, value)| !(named && fallback(key, value)))
        .map(|(key, value)| KeyValue::new(key.clone(), value.clone()))
        .collect();
    // Laid last: the SDK's detectors would let a `service.name` in the
    // attributes win over it, against the specification.
    let service_name = service_name.map(|name| KeyValue::new(SERVICE_NAME, name));
    Ok(Resource::builder_empty()
        .with_schema_url(Vec::<KeyValue>::new(), SCHEMA_URL)
        .with_attributes(ours)
        .with_attributes(over)
        .with_attributes(
            attributes
                .into_iter()
                .map(|(key, value)| KeyValue::new(key, value)),
        )
        .with_attributes(service_name)
        .build())
}

#[cfg(test)]
#[expect(
    clippy::let_underscore_must_use,
    clippy::result_large_err,
    reason = "the tests shut the providers they built down best-effort, and figment::Jail fixes \
              the closure's error type"
)]
mod tests {
    use opentelemetry::Key;
    use opentelemetry_sdk::metrics::OTEL_METRIC_EXPORT_INTERVAL_DEFAULT;

    use super::*;
    use crate::config::{OTEL_RESOURCE_ATTRIBUTES, OTEL_SERVICE_NAME, PINNED_SERVICE_NAME};
    use crate::testing::reading;

    /// `build` mutates `opentelemetry::global`, so every `build()` in this file is
    /// serialized.
    static GLOBAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn nothing(_: &str) -> Option<String> {
        None
    }

    fn binary() -> AppIdentity {
        nest_rs_core::__private::package_identity("auth", "7.0.0", "")
    }

    fn pinned_endpoint(endpoint: &str) -> OpenTelemetryConfig {
        OpenTelemetryConfig {
            otlp_endpoint: Some(endpoint.into()),
            ..OpenTelemetryConfig::default()
        }
    }

    fn shut(exporters: Exporters) {
        let _ = exporters.tracer_provider.shutdown();
        if let Some(p) = exporters.meter_provider {
            let _ = p.shutdown();
        }
        if let Some(p) = exporters.logger_provider {
            let _ = p.shutdown();
        }
    }

    /// The three destinations as the boot line says them.
    fn said(exporters: &Exporters) -> [String; 3] {
        exporters
            .summary
            .destinations
            .each_ref()
            .map(ToString::to_string)
    }

    fn attribute(resource: &Resource, key: &'static str) -> Option<String> {
        resource.get(&Key::new(key)).map(|v| v.as_str().to_string())
    }

    #[test]
    fn the_service_is_named_and_versioned_after_the_binary_unless_pinned() {
        figment::Jail::expect_with(|_| {
            let r = build_resource(
                &OpenTelemetryConfig::default(),
                Some(&binary()),
                &nest_rs_config::env_var,
            )
            .expect("readable");
            assert_eq!(attribute(&r, SERVICE_NAME).as_deref(), Some("auth"));
            assert_eq!(attribute(&r, SERVICE_VERSION).as_deref(), Some("7.0.0"));

            let pinned = OpenTelemetryConfig {
                service_name: Some("configured-svc".into()),
                service_version: Some("1.2.3".into()),
                ..OpenTelemetryConfig::default()
            };
            let r = build_resource(&pinned, Some(&binary()), &nest_rs_config::env_var)
                .expect("readable");
            assert_eq!(
                attribute(&r, SERVICE_NAME).as_deref(),
                Some("configured-svc")
            );
            assert_eq!(attribute(&r, SERVICE_VERSION).as_deref(), Some("1.2.3"));
            Ok(())
        });
    }

    /// `SdkProvidedResourceDetector` supplies `unknown_service:<binary>` when no
    /// variable names the service: a fallback, kept only when nothing else names it.
    #[test]
    fn with_nothing_naming_the_service_the_sdk_s_fallback_stands() {
        figment::Jail::expect_with(|_| {
            let r = build_resource(
                &OpenTelemetryConfig::default(),
                None,
                &nest_rs_config::env_var,
            )
            .expect("readable");
            let name = attribute(&r, SERVICE_NAME).expect("the SDK always names one");
            assert!(name.starts_with("unknown_service"), "{name}");
            assert!(attribute(&r, SERVICE_VERSION).is_none());
            Ok(())
        });
    }

    /// What the specification's variables set wins over the pin and the
    /// binary: an operator's `OTEL_SERVICE_NAME` is the name.
    #[test]
    fn the_specification_s_variables_name_the_service_over_the_pin_and_the_binary() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("OTEL_SERVICE_NAME", "from-the-operator");
            jail.set_env(
                OTEL_RESOURCE_ATTRIBUTES,
                "service.version=9.9.9,deployment.environment.name=staging",
            );
            let pinned = OpenTelemetryConfig {
                service_name: Some("pinned".into()),
                service_version: Some("1.0.0".into()),
                deployment_environment: Some("production".into()),
                ..OpenTelemetryConfig::default()
            };
            let r = build_resource(&pinned, Some(&binary()), &nest_rs_config::env_var)
                .expect("readable");
            assert_eq!(
                attribute(&r, SERVICE_NAME).as_deref(),
                Some("from-the-operator")
            );
            assert_eq!(attribute(&r, SERVICE_VERSION).as_deref(), Some("9.9.9"));
            assert_eq!(
                attribute(&r, DEPLOYMENT_ENVIRONMENT_NAME).as_deref(),
                Some("staging")
            );
            Ok(())
        });
        figment::Jail::expect_with(|jail| {
            jail.set_env(OTEL_RESOURCE_ATTRIBUTES, "service.name=from-attributes");
            let r = build_resource(
                &OpenTelemetryConfig::default(),
                Some(&binary()),
                &nest_rs_config::env_var,
            )
            .expect("readable");
            assert_eq!(
                attribute(&r, SERVICE_NAME).as_deref(),
                Some("from-attributes")
            );
            Ok(())
        });
    }

    /// The specification's own precedence: `OTEL_SERVICE_NAME` over a
    /// `service.name` in `OTEL_RESOURCE_ATTRIBUTES`, which the SDK's detectors
    /// read the other way round.
    #[test]
    fn otel_service_name_wins_over_service_name_in_the_resource_attributes() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("OTEL_SERVICE_NAME", "from-service-name");
            jail.set_env(OTEL_RESOURCE_ATTRIBUTES, "service.name=from-attributes");
            let r = build_resource(
                &OpenTelemetryConfig::default(),
                Some(&binary()),
                &nest_rs_config::env_var,
            )
            .expect("readable");
            assert_eq!(
                attribute(&r, SERVICE_NAME).as_deref(),
                Some("from-service-name")
            );
            Ok(())
        });
    }

    /// `OTEL_RESOURCE_ATTRIBUTES` is read as the resource specification
    /// writes it — each key and value trimmed and percent-decoded, a blank
    /// member skipped — where the SDK alone exports an escape as written.
    #[test]
    fn the_resource_attributes_are_percent_decoded_as_the_specification_writes_them() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(
                OTEL_RESOURCE_ATTRIBUTES,
                " team = a%2Cb ,, deployment.environment.name=caf%C3%A9,k%3Dy=v%3Dw",
            );
            let r = build_resource(
                &OpenTelemetryConfig::default(),
                Some(&binary()),
                &nest_rs_config::env_var,
            )
            .expect("readable");
            assert_eq!(attribute(&r, "team").as_deref(), Some("a,b"));
            assert_eq!(
                attribute(&r, DEPLOYMENT_ENVIRONMENT_NAME).as_deref(),
                Some("caf\u{e9}")
            );
            assert_eq!(attribute(&r, "k=y").as_deref(), Some("v=w"));
            Ok(())
        });
    }

    /// A member the SDK would drop in silence (no `=`) or keep under no name
    /// (an empty key), and an escape that does not decode, where the
    /// specification discards the whole variable, refuse the build naming
    /// `OTEL_RESOURCE_ATTRIBUTES`, never quoting it.
    #[test]
    fn a_resource_attribute_the_sdk_would_drop_or_misread_is_refused_naming_the_variable() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for value in [
            "s3cr3t",
            "deployment.environment.name=prod,s3cr3t",
            "=s3cr3t",
            " =s3cr3t",
            "team=s3cr3t%zz",
            "team=s3cr3t%C3",
            "te%Cam=s3cr3t",
        ] {
            let refused = match build_reading(
                &OpenTelemetryConfig::default(),
                None,
                reading(&[(OTEL_RESOURCE_ATTRIBUTES, value)]),
            ) {
                Ok(exporters) => {
                    shut(exporters);
                    panic!("{value:?} must not build");
                }
                Err(refused) => refused.to_string(),
            };
            assert!(
                refused.contains(OTEL_RESOURCE_ATTRIBUTES) && refused.contains("key=value"),
                "{refused}"
            );
            assert!(
                !refused.contains("s3cr3t") && !refused.contains("team"),
                "{refused}"
            );
        }
    }

    #[test]
    fn the_instance_id_is_pinned_or_a_fresh_v7_uuid_per_build() {
        figment::Jail::expect_with(|_| {
            let unpinned = OpenTelemetryConfig::default();
            let a = attribute(
                &build_resource(&unpinned, None, &nest_rs_config::env_var).expect("readable"),
                SERVICE_INSTANCE_ID,
            )
            .expect("instance id present");
            let b = attribute(
                &build_resource(&unpinned, None, &nest_rs_config::env_var).expect("readable"),
                SERVICE_INSTANCE_ID,
            )
            .expect("instance id present");
            assert!(Uuid::parse_str(&a).is_ok(), "parses as a uuid: {a}");
            assert_ne!(a, b, "each invocation mints a fresh id");

            let pinned = OpenTelemetryConfig {
                service_instance_id: Some("pinned-instance".into()),
                ..OpenTelemetryConfig::default()
            };
            assert_eq!(
                attribute(
                    &build_resource(&pinned, None, &nest_rs_config::env_var).expect("readable"),
                    SERVICE_INSTANCE_ID
                )
                .as_deref(),
                Some("pinned-instance")
            );
            Ok(())
        });
    }

    #[test]
    fn nothing_setting_an_endpoint_installs_only_the_local_tracer() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let exporters =
            build_reading(&OpenTelemetryConfig::default(), None, nothing).expect("builds");
        assert!(exporters.meter_provider.is_none() && exporters.logger_provider.is_none());
        assert_eq!(said(&exporters), ["off (no endpoint)"; 3]);
        shut(exporters);
    }

    #[test]
    fn a_pinned_base_endpoint_exports_every_signal_at_its_path() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for base in ["http://localhost:4318", "http://localhost:4318/"] {
            let exporters = build_reading(&pinned_endpoint(base), None, nothing).expect("builds");
            assert!(exporters.meter_provider.is_some() && exporters.logger_provider.is_some());
            assert_eq!(
                said(&exporters),
                [
                    "http://localhost:4318/v1/traces",
                    "http://localhost:4318/v1/metrics",
                    "http://localhost:4318/v1/logs",
                ]
            );
            shut(exporters);
        }
    }

    /// `OTEL_EXPORTER_OTLP_ENDPOINT` is the base every signal appends its path
    /// to, over the pin; a signal's own variable is used as given, over both.
    #[test]
    fn the_specification_s_endpoints_override_the_pinned_one_signal_by_signal() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let exporters = build_reading(
            &pinned_endpoint("http://pinned:4318"),
            None,
            reading(&[
                (OTEL_EXPORTER_OTLP_ENDPOINT, "http://collector:4318"),
                (OTEL_EXPORTER_OTLP_LOGS_ENDPOINT, "http://logs:9000/ingest"),
            ]),
        )
        .expect("builds");
        assert_eq!(
            said(&exporters),
            [
                "http://collector:4318/v1/traces",
                "http://collector:4318/v1/metrics",
                "http://logs:9000/ingest",
            ]
        );
        shut(exporters);

        let exporters = build_reading(
            &OpenTelemetryConfig::default(),
            None,
            reading(&[(
                OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                "http://traces:4318/v1/traces",
            )]),
        )
        .expect("builds");
        assert!(
            exporters.meter_provider.is_none() && exporters.logger_provider.is_none(),
            "a signal no endpoint names exports nothing"
        );
        assert_eq!(said(&exporters)[0], "http://traces:4318/v1/traces");
        shut(exporters);
    }

    /// OTLP/HTTP is spoken over http or https, and an endpoint that does not
    /// parse is refused: each named by what set it, never quoted, before any
    /// exporter is built.
    #[test]
    fn an_endpoint_refused_names_its_variable_or_field_and_never_its_value() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let refusal =
            |config: &OpenTelemetryConfig, read: fn(&str) -> Option<String>| match build_reading(
                config, None, read,
            ) {
                Ok(_) => panic!("a refused endpoint must not build"),
                Err(refused) => refused.to_string(),
            };
        let said = refusal(&pinned_endpoint("grpc://collector:4317"), nothing);
        assert!(
            said.contains(PINNED_ENDPOINT) && said.contains("must be an http:// or https:// URL"),
            "{said}"
        );
        let said = refusal(&OpenTelemetryConfig::default(), |name| {
            (name == OTEL_EXPORTER_OTLP_ENDPOINT).then(|| "https://u:s3cr3t@[::1".to_owned())
        });
        assert!(
            said.contains(OTEL_EXPORTER_OTLP_ENDPOINT) && said.contains("does not parse as a URL"),
            "{said}"
        );
        assert!(!said.contains("s3cr3t"), "{said}");
    }

    /// The exporter speaks http/protobuf alone; a deployment naming another
    /// protocol points at a port that speaks it, so it is refused, never
    /// overridden in silence.
    #[test]
    fn a_protocol_other_than_http_protobuf_is_refused_naming_its_variable() {
        let config = pinned_endpoint("http://collector:4318");
        assert!(
            export(
                Signal::Traces,
                &config,
                &reading(&[(OTEL_EXPORTER_OTLP_PROTOCOL, "http/protobuf")]),
            )
            .expect("the one it speaks")
            .is_some()
        );
        for (variable, protocol) in [
            (OTEL_EXPORTER_OTLP_PROTOCOL, "grpc"),
            (OTEL_EXPORTER_OTLP_METRICS_PROTOCOL, "http/json"),
        ] {
            let read = move |name: &str| (name == variable).then(|| protocol.to_owned());
            let refused = export(Signal::Metrics, &config, &read)
                .err()
                .expect("another protocol")
                .to_string();
            assert!(
                refused.contains(variable) && refused.contains("http/protobuf"),
                "{refused}"
            );
        }
    }

    #[test]
    fn the_sdk_s_sampler_variable_replaces_the_pinned_ratio_and_a_bad_ratio_is_refused() {
        let pinned = OpenTelemetryConfig {
            trace_sample_ratio: 0.25,
            ..OpenTelemetryConfig::default()
        };
        let chosen = |set: &'static [(&'static str, &'static str)]| {
            let mut passed_over = Vec::new();
            let (sampler, choice) = sampler(&pinned, &reading(set), &mut passed_over)
                .unwrap_or_else(|refused| panic!("{set:?}: {refused}"));
            let passed_over: Vec<&str> = passed_over.iter().map(|over| over.variable).collect();
            (
                format!("{sampler:?}"),
                choice.source == OTEL_TRACES_SAMPLER,
                passed_over,
            )
        };
        let parent_based = |inner: &str| format!("ParentBased({inner})");
        assert_eq!(
            chosen(&[]),
            (parent_based("TraceIdRatioBased(0.25)"), false, vec![])
        );
        assert_eq!(
            chosen(&[(OTEL_TRACES_SAMPLER, "always_off")]),
            ("AlwaysOff".to_owned(), true, vec![]),
            "OTEL_TRACES_SAMPLER stands over the pinned ratio"
        );
        assert_eq!(
            chosen(&[
                (OTEL_TRACES_SAMPLER, "parentbased_traceidratio"),
                (OTEL_TRACES_SAMPLER_ARG, "0.1"),
            ]),
            (parent_based("TraceIdRatioBased(0.1)"), true, vec![])
        );
        assert_eq!(
            chosen(&[(OTEL_TRACES_SAMPLER, "traceidratio")]),
            ("TraceIdRatioBased(1.0)".to_owned(), true, vec![]),
            "the specification's default argument"
        );
        assert_eq!(
            chosen(&[
                (OTEL_TRACES_SAMPLER, "parentbased_always_on"),
                (OTEL_TRACES_SAMPLER_ARG, "0.1"),
            ]),
            (
                parent_based("AlwaysOn"),
                true,
                vec![OTEL_TRACES_SAMPLER_ARG]
            ),
            "an argument the sampler takes none of is passed over and said"
        );
        assert_eq!(
            chosen(&[(OTEL_TRACES_SAMPLER_ARG, "0.1")]),
            (
                parent_based("TraceIdRatioBased(0.25)"),
                false,
                vec![OTEL_TRACES_SAMPLER_ARG]
            ),
        );
        for ratio in [f64::NAN, -0.5, 2.5] {
            let refused = sampler(
                &OpenTelemetryConfig {
                    trace_sample_ratio: ratio,
                    ..OpenTelemetryConfig::default()
                },
                &nothing,
                &mut Vec::new(),
            )
            .expect_err("outside [0, 1]")
            .to_string();
            assert!(
                refused.contains(PINNED_RATIO) && refused.contains("between 0 and 1"),
                "{refused}"
            );
        }
    }

    #[test]
    fn the_metric_period_is_the_sdk_s_variable_in_milliseconds_over_the_pinned_one() {
        assert_eq!(
            crate::DEFAULT_METRIC_INTERVAL,
            OTEL_METRIC_EXPORT_INTERVAL_DEFAULT,
            "the SDK's own period, named rather than implicit",
        );
        let pinned = OpenTelemetryConfig {
            metric_interval: Duration::from_secs(30),
            ..OpenTelemetryConfig::default()
        };
        assert_eq!(
            metric_interval(&pinned, &nothing).expect("in range"),
            Duration::from_secs(30)
        );
        assert_eq!(
            metric_interval(&pinned, &reading(&[(OTEL_METRIC_EXPORT_INTERVAL, "5000")]))
                .expect("in range"),
            Duration::from_secs(5)
        );

        for (value, sentence) in [
            ("5s", "whole number of milliseconds"),
            ("0", "below the 1s it must be at least"),
        ] {
            let refused =
                metric_interval(&pinned, &reading(&[(OTEL_METRIC_EXPORT_INTERVAL, value)]))
                    .expect_err(value)
                    .to_string();
            assert!(
                refused.contains(OTEL_METRIC_EXPORT_INTERVAL) && refused.contains(sentence),
                "{refused}"
            );
        }
        let refused = metric_interval(
            &OpenTelemetryConfig {
                metric_interval: Duration::ZERO,
                ..OpenTelemetryConfig::default()
            },
            &nothing,
        )
        .expect_err("a pinned zero is held where it is spent")
        .to_string();
        assert!(refused.contains(PINNED_INTERVAL), "{refused}");
    }

    /// The SDK's order: the signal's variable, then the general one, in
    /// milliseconds, then its 10 s; a value that is not a whole number of
    /// milliseconds is refused naming its variable, never quoted, where the
    /// SDK would pass it over.
    #[test]
    fn an_export_timeout_is_resolved_as_the_sdk_resolves_it() {
        let resolved = |set: &'static [(&'static str, &'static str)]| {
            [
                OTEL_EXPORTER_OTLP_TRACES_TIMEOUT,
                OTEL_EXPORTER_OTLP_METRICS_TIMEOUT,
                OTEL_EXPORTER_OTLP_LOGS_TIMEOUT,
            ]
            .map(|signal| {
                export_timeout(signal, &reading(set))
                    .expect("milliseconds")
                    .1
                    .as_millis()
            })
        };

        assert_eq!(resolved(&[]), [10_000; 3]);
        assert_eq!(resolved(&[(OTEL_EXPORTER_OTLP_TIMEOUT, "2000")]), [2000; 3]);
        assert_eq!(
            resolved(&[
                (OTEL_EXPORTER_OTLP_TIMEOUT, "2000"),
                (OTEL_EXPORTER_OTLP_METRICS_TIMEOUT, " 300 "),
            ]),
            [2000, 300, 2000]
        );
        for (set, variable) in [
            (
                &[
                    (OTEL_EXPORTER_OTLP_TIMEOUT, "2000"),
                    (OTEL_EXPORTER_OTLP_LOGS_TIMEOUT, "5s"),
                ][..],
                OTEL_EXPORTER_OTLP_LOGS_TIMEOUT,
            ),
            (
                &[(OTEL_EXPORTER_OTLP_TIMEOUT, "-1")][..],
                OTEL_EXPORTER_OTLP_TIMEOUT,
            ),
        ] {
            let refused = export_timeout(OTEL_EXPORTER_OTLP_LOGS_TIMEOUT, &reading(set))
                .expect_err("not milliseconds")
                .to_string();
            assert!(
                refused.contains(variable) && refused.contains(MILLISECONDS),
                "{refused}"
            );
            assert!(
                !refused.contains("5s") && !refused.contains("-1"),
                "{refused}"
            );
        }
    }

    /// A file `name` holding `contents` in `scratch`, which the test holds.
    fn written(scratch: &tempfile::TempDir, name: &str, contents: &str) -> String {
        let path = scratch.path().join(name);
        std::fs::write(&path, contents).expect("a scratch file");
        path.to_string_lossy().into_owned()
    }

    /// The specification names the certificate and its key apart, per signal
    /// or for all three: half a pair is refused naming what was set and both
    /// spellings of what is missing.
    #[test]
    fn half_a_client_certificate_is_refused_naming_both_settings() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let cert = written(&scratch, "client.pem", "-----BEGIN CERTIFICATE-----");
        let read = move |name: &str| {
            (name == "OTEL_EXPORTER_OTLP_TRACES_CLIENT_CERTIFICATE").then(|| cert.clone())
        };
        let Err(refused) = signal_tls(Signal::Traces, &ClientTls::default(), &read) else {
            panic!("a certificate without its key must not build");
        };
        let said = refused.to_string();
        assert!(
            said.contains("OTEL_EXPORTER_OTLP_TRACES_CLIENT_CERTIFICATE")
                && said.contains(
                    "OTEL_EXPORTER_OTLP_TRACES_CLIENT_KEY (or OTEL_EXPORTER_OTLP_CLIENT_KEY)"
                ),
            "{said}"
        );
    }

    #[test]
    fn a_certificate_file_that_cannot_be_read_is_refused_naming_its_variable() {
        let read = |name: &str| {
            (name == OTEL_EXPORTER_OTLP_CERTIFICATE)
                .then(|| "/nonexistent/nestrs/collector-ca.pem".to_owned())
        };
        let Err(refused) = signal_tls(Signal::Metrics, &ClientTls::default(), &read) else {
            panic!("a file that cannot be read must not build");
        };
        assert!(
            matches!(&refused, ConfigError::File { var, .. } if var == OTEL_EXPORTER_OTLP_CERTIFICATE),
            "{refused}"
        );
    }

    /// An `http://` endpoint declares the export in plaintext: an authority
    /// beside it would go unused, and is refused naming both.
    #[test]
    fn an_authority_beside_a_plaintext_endpoint_is_refused_naming_both() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let authority = written(&scratch, "authority.pem", "-----BEGIN CERTIFICATE-----");
        let read = move |name: &str| match name {
            OTEL_EXPORTER_OTLP_ENDPOINT => Some("http://127.0.0.1:4318".to_owned()),
            OTEL_EXPORTER_OTLP_CERTIFICATE => Some(authority.clone()),
            _ => None,
        };
        let Err(refused) = build_reading(&OpenTelemetryConfig::default(), None, read) else {
            panic!("TLS material beside a plaintext endpoint must not build");
        };
        let said = refused.to_string();
        assert!(
            said.contains(OTEL_EXPORTER_OTLP_CERTIFICATE)
                && said.contains(OTEL_EXPORTER_OTLP_ENDPOINT),
            "{said}"
        );
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

        let Err(cut) = flushed else {
            panic!("the final export never answered, so the flush cannot have succeeded");
        };
        // The SDK's own word for where it stopped waiting, which no stall moves.
        let at_the_bound = format!(
            "{:?}",
            opentelemetry_sdk::error::OTelSdkError::Timeout(bound)
        );
        assert!(
            cut.to_string().contains(&at_the_bound),
            "shutdown gave the export its bound and no more: {cut}",
        );
        assert!(took >= bound, "and gave it all of it: took {took:?}");
    }

    /// The specification's exporter selectors, which no crate of the SDK names.
    const SELECTORS: [&str; 3] = [
        "OTEL_TRACES_EXPORTER",
        "OTEL_METRICS_EXPORTER",
        "OTEL_LOGS_EXPORTER",
    ];

    /// `none` turns its signal off whatever endpoint names it, in any case, and
    /// leaves the others exporting.
    #[test]
    fn a_selector_set_to_none_switches_its_signal_off_whatever_the_endpoint() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let exporters = build_reading(
            &OpenTelemetryConfig::default(),
            None,
            reading(&[
                (OTEL_EXPORTER_OTLP_ENDPOINT, "http://collector:4318"),
                ("OTEL_METRICS_EXPORTER", "none"),
                ("OTEL_LOGS_EXPORTER", " None "),
            ]),
        )
        .expect("builds");
        assert!(
            exporters.meter_provider.is_none() && exporters.logger_provider.is_none(),
            "a signal switched off builds no exporter"
        );
        assert_eq!(said(&exporters)[0], "http://collector:4318/v1/traces");
        shut(exporters);
    }

    /// `otlp` keeps a signal its endpoint exports, named in any case, alone or
    /// repeated in the specification's comma-separated list.
    #[test]
    fn otlp_alone_or_in_a_list_keeps_a_signal_its_endpoint_exports() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let exporters = build_reading(
            &pinned_endpoint("http://collector:4318"),
            None,
            reading(&[
                ("OTEL_TRACES_EXPORTER", "otlp"),
                ("OTEL_METRICS_EXPORTER", "OTLP"),
                ("OTEL_LOGS_EXPORTER", "otlp, otlp"),
            ]),
        )
        .expect("builds");
        assert!(exporters.meter_provider.is_some() && exporters.logger_provider.is_some());
        shut(exporters);
    }

    /// The refusal `build` fails with under `set`, as said.
    fn refused_under(set: &[(&str, &str)]) -> String {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Err(refused) = build_reading(
            &pinned_endpoint("http://collector:4318"),
            None,
            reading(set),
        ) else {
            panic!("{set:?} must not build");
        };
        refused.to_string()
    }

    /// An exporter this crate does not build — one the specification knows, a
    /// misspelling — is refused naming its selector and what it accepts, never
    /// the value: a deployment believing it ships where nothing does is the
    /// defect a warning would leave.
    #[test]
    fn an_exporter_this_crate_does_not_build_is_refused_naming_its_selector_and_never_the_value() {
        for variable in SELECTORS {
            for value in [
                "console",
                "zipkin",
                "prometheus",
                "logging",
                "otlp/stdout",
                "otpl",
                "otlp,zipkin",
            ] {
                let said = refused_under(&[(variable, value)]);
                assert!(
                    said.contains(variable) && said.contains("otlp") && said.contains("none"),
                    "{said}"
                );
                assert!(!said.contains(value), "{said}");
            }
            assert!(
                refused_under(&[(variable, " , ")]).contains(variable),
                "a list naming nothing"
            );
        }
    }

    /// `none` turns the signal's exporters off, so it stands alone.
    #[test]
    fn none_beside_otlp_is_refused() {
        for value in ["otlp,none", "none, OTLP"] {
            let said = refused_under(&[("OTEL_LOGS_EXPORTER", value)]);
            assert!(
                said.contains("OTEL_LOGS_EXPORTER") && said.contains("stands alone"),
                "{said}"
            );
        }
    }

    /// `otlp` named for a signal nothing gives an endpoint is refused naming
    /// where one is set: an unset selector, the specification's default, stays
    /// quiet instead.
    #[test]
    fn otlp_named_for_a_signal_with_no_endpoint_is_refused_naming_where_one_is_set() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Err(refused) = build_reading(
            &OpenTelemetryConfig::default(),
            None,
            reading(&[
                (
                    OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                    "http://traces:4318/v1/traces",
                ),
                ("OTEL_METRICS_EXPORTER", "otlp"),
            ]),
        ) else {
            panic!("otlp with no endpoint must not build");
        };
        let said = refused.to_string();
        assert!(
            said.contains("OTEL_METRICS_EXPORTER")
                && said.contains(OTEL_EXPORTER_OTLP_METRICS_ENDPOINT)
                && said.contains(OTEL_EXPORTER_OTLP_ENDPOINT)
                && said.contains(PINNED_ENDPOINT),
            "{said}"
        );
    }

    /// `OTEL_SDK_DISABLED=true`, in any case, turns every signal off whatever the
    /// endpoints and the selectors say; a selector that does not parse is still
    /// refused.
    #[test]
    fn sdk_disabled_switches_every_signal_off() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for disabled in ["true", "TRUE", " True "] {
            let exporters = build_reading(
                &OpenTelemetryConfig::default(),
                None,
                reading(&[
                    (OTEL_EXPORTER_OTLP_ENDPOINT, "http://collector:4318"),
                    ("OTEL_SDK_DISABLED", disabled),
                ]),
            )
            .expect("builds");
            assert!(
                exporters.meter_provider.is_none() && exporters.logger_provider.is_none(),
                "{disabled:?} disables the SDK"
            );
            assert_eq!(said(&exporters), ["off (OTEL_SDK_DISABLED=true)"; 3]);
            assert!(exporters.summary.passed_over.is_empty());
            shut(exporters);
        }
        let exporters = build_reading(
            &OpenTelemetryConfig::default(),
            None,
            reading(&[
                ("OTEL_SDK_DISABLED", "true"),
                ("OTEL_LOGS_EXPORTER", "otlp"),
            ]),
        )
        .expect("otlp with no endpoint is no refusal once the SDK is disabled");
        shut(exporters);
        let Err(refused) = build_reading(
            &OpenTelemetryConfig::default(),
            None,
            reading(&[
                ("OTEL_SDK_DISABLED", "true"),
                ("OTEL_TRACES_EXPORTER", "zipkin"),
            ]),
        ) else {
            panic!("a selector that does not parse is refused under OTEL_SDK_DISABLED");
        };
        let said = refused.to_string();
        assert!(said.contains("OTEL_TRACES_EXPORTER"), "{said}");
    }

    /// Any value but `true` leaves the SDK on, as the specification orders; one
    /// neither true nor false is said by its variable.
    #[test]
    fn sdk_disabled_other_than_true_leaves_the_sdk_on_and_a_stray_value_is_said() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for (value, said) in [
            ("false", false),
            ("FALSE", false),
            ("1", true),
            ("yes", true),
        ] {
            let exporters = build_reading(
                &OpenTelemetryConfig::default(),
                None,
                reading(&[
                    (OTEL_EXPORTER_OTLP_ENDPOINT, "http://collector:4318"),
                    ("OTEL_SDK_DISABLED", value),
                ]),
            )
            .expect("builds");
            assert!(
                exporters.meter_provider.is_some() && exporters.logger_provider.is_some(),
                "{value:?} leaves the SDK on"
            );
            assert_eq!(
                exporters
                    .summary
                    .passed_over
                    .iter()
                    .any(|over| over.variable == "OTEL_SDK_DISABLED"),
                said,
                "{value:?}"
            );
            shut(exporters);
        }
    }

    /// The boot line tells a signal its selector switched off from one no
    /// endpoint names.
    #[test]
    fn the_boot_line_tells_a_signal_switched_off_from_one_with_no_endpoint() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let exporters = build_reading(
            &OpenTelemetryConfig::default(),
            None,
            reading(&[
                (
                    OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                    "http://traces:4318/v1/traces",
                ),
                (
                    OTEL_EXPORTER_OTLP_METRICS_ENDPOINT,
                    "http://metrics:4318/v1/metrics",
                ),
                ("OTEL_TRACES_EXPORTER", "none"),
            ]),
        )
        .expect("builds");
        assert_eq!(
            said(&exporters),
            [
                "off (OTEL_TRACES_EXPORTER=none)",
                "http://metrics:4318/v1/metrics",
                "off (no endpoint)",
            ]
        );
        shut(exporters);
    }

    /// What reaches a collector over plain HTTP off this machine, signal by
    /// signal; a collector beside the app, by any name of the loopback, crosses
    /// no network, and a signal switched off exports nothing to say.
    #[test]
    fn a_signal_exporting_in_plaintext_off_this_machine_is_summed_up_alone() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let in_plaintext = |config: &OpenTelemetryConfig, set: &[(&str, &str)]| {
            let exporters = build_reading(config, None, reading(set)).expect("builds");
            let found: Vec<(&str, String)> = exporters
                .summary
                .in_plaintext
                .iter()
                .map(|(signal, endpoint)| (signal.name(), endpoint.clone()))
                .collect();
            shut(exporters);
            found
        };
        let none = OpenTelemetryConfig::default();
        assert!(in_plaintext(&none, &[]).is_empty());
        for loopback in [
            "http://localhost:4318",
            "http://127.0.0.1:4318",
            "http://[::1]:4318",
        ] {
            assert!(
                in_plaintext(&pinned_endpoint(loopback), &[]).is_empty(),
                "{loopback}"
            );
        }
        assert!(in_plaintext(&pinned_endpoint("https://collector:4318"), &[]).is_empty());
        assert_eq!(
            in_plaintext(&pinned_endpoint("http://collector:4318"), &[]),
            ["traces", "metrics", "logs"]
                .map(|signal| (signal, format!("http://collector:4318/v1/{signal}"),)),
        );
        assert_eq!(
            in_plaintext(
                &none,
                &[
                    (
                        OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
                        "http://traces:4318/v1/traces"
                    ),
                    (OTEL_EXPORTER_OTLP_METRICS_ENDPOINT, "https://metrics:4318"),
                    (OTEL_EXPORTER_OTLP_LOGS_ENDPOINT, "http://127.0.0.1:4318"),
                ],
            ),
            [("traces", "http://traces:4318/v1/traces".to_owned())],
        );
        assert_eq!(
            in_plaintext(
                &pinned_endpoint("http://collector:4318"),
                &[
                    ("OTEL_TRACES_EXPORTER", "none"),
                    ("OTEL_METRICS_EXPORTER", "none"),
                ],
            ),
            [("logs", "http://collector:4318/v1/logs".to_owned())],
        );
        assert!(
            in_plaintext(
                &pinned_endpoint("http://collector:4318"),
                &[(OTEL_SDK_DISABLED, "true")],
            )
            .is_empty()
        );
    }

    /// A sampler the SDK does not build, and a ratio that does not read as one
    /// between 0 and 1, are refused naming their variable, never the value.
    #[test]
    fn a_sampler_or_a_ratio_the_sdk_would_pass_over_is_refused_naming_it() {
        for (set, variable, value) in [
            (
                &[(OTEL_TRACES_SAMPLER, "jaeger_remote")][..],
                OTEL_TRACES_SAMPLER,
                "jaeger_remote",
            ),
            (
                &[(OTEL_TRACES_SAMPLER, "xray")][..],
                OTEL_TRACES_SAMPLER,
                "xray",
            ),
            (
                &[(OTEL_TRACES_SAMPLER, "Always_On")][..],
                OTEL_TRACES_SAMPLER,
                "Always_On",
            ),
            (
                &[
                    (OTEL_TRACES_SAMPLER, "traceidratio"),
                    (OTEL_TRACES_SAMPLER_ARG, "10%"),
                ][..],
                OTEL_TRACES_SAMPLER_ARG,
                "10%",
            ),
            (
                &[
                    (OTEL_TRACES_SAMPLER, "parentbased_traceidratio"),
                    (OTEL_TRACES_SAMPLER_ARG, "10"),
                ][..],
                OTEL_TRACES_SAMPLER_ARG,
                "10",
            ),
        ] {
            let Err(refused) = sampler(
                &OpenTelemetryConfig::default(),
                &reading(set),
                &mut Vec::new(),
            ) else {
                panic!("{set:?} must not build");
            };
            let said = refused.to_string();
            assert!(said.contains(variable), "{said}");
            assert!(value == "10" || !said.contains(value), "{said}");
        }
    }

    /// The SDK's own parser reads the temporality, in any case; anything else
    /// is refused naming the variable.
    #[test]
    fn a_metric_temporality_is_the_sdk_s_or_refused_naming_its_variable() {
        let variable = OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE;
        assert_eq!(temporality(&nothing).expect("unset"), None);
        for (value, read) in [
            ("cumulative", Temporality::Cumulative),
            ("Delta", Temporality::Delta),
            ("LOWMEMORY", Temporality::LowMemory),
        ] {
            assert_eq!(
                temporality(&reading(&[(variable, value)])).expect(value),
                Some(read)
            );
        }
        for value in ["deltas", " delta", "gauge"] {
            let said = temporality(&reading(&[(variable, value)]))
                .expect_err(value)
                .to_string();
            assert!(
                said.contains(variable) && said.contains("cumulative"),
                "{said}"
            );
            assert!(value.trim() == "delta" || !said.contains(value), "{said}");
        }
    }

    /// `gzip` or `none`, in any case, a signal's own variable over the general
    /// one, as the specification orders them.
    #[test]
    fn compression_is_gzip_or_none_a_signal_s_own_over_the_general_one() {
        let resolved = |set: &'static [(&'static str, &'static str)]| {
            Signal::ALL.map(|signal| compression(signal, &reading(set)).expect("readable"))
        };
        assert_eq!(resolved(&[]), [None; 3]);
        assert_eq!(
            resolved(&[(OTEL_EXPORTER_OTLP_COMPRESSION, "gzip")]),
            [Some(Compression::Gzip); 3]
        );
        assert_eq!(
            resolved(&[
                (OTEL_EXPORTER_OTLP_COMPRESSION, "GZIP"),
                (OTEL_EXPORTER_OTLP_METRICS_COMPRESSION, "None"),
            ]),
            [Some(Compression::Gzip), None, Some(Compression::Gzip)]
        );
        assert_eq!(
            resolved(&[
                (OTEL_EXPORTER_OTLP_COMPRESSION, "none"),
                (OTEL_EXPORTER_OTLP_LOGS_COMPRESSION, "gzip"),
            ]),
            [None, None, Some(Compression::Gzip)]
        );
    }

    /// A compression this exporter does not build — the SDK's `zstd`, another
    /// algorithm, a padded value the SDK would read apart — is refused naming
    /// the variable it was read from, never the value.
    #[test]
    fn a_compression_this_exporter_does_not_build_is_refused_naming_its_variable() {
        for (variable, value) in [
            (OTEL_EXPORTER_OTLP_COMPRESSION, "zstd"),
            (OTEL_EXPORTER_OTLP_TRACES_COMPRESSION, "brotli"),
            (OTEL_EXPORTER_OTLP_COMPRESSION, " gzip"),
        ] {
            let said = compression(Signal::Traces, &reading(&[(variable, value)]))
                .expect_err(value)
                .to_string();
            assert!(
                said.contains(variable) && said.contains("must be gzip"),
                "{said}"
            );
            assert!(value.trim() == "gzip" || !said.contains(value), "{said}");
        }
    }

    /// The export `signal` resolves to under `set`, the pinned endpoint
    /// `pinned`, or its refusal.
    fn resolved(
        signal: Signal,
        pinned: Option<&str>,
        set: &[(&str, &str)],
    ) -> Result<Option<Export>, ConfigError> {
        let config = pinned.map_or_else(OpenTelemetryConfig::default, pinned_endpoint);
        export(signal, &config, &reading(set))
    }

    /// The bound, in milliseconds, on one export of each signal — traces,
    /// metrics, logs — to a collector under `set`.
    fn export_timeouts(set: &[(&str, &str)]) -> [u128; 3] {
        Signal::ALL.map(|signal| {
            resolved(signal, Some("http://collector:4318"), set)
                .expect("builds")
                .expect("exports")
                .timeout
                .as_millis()
        })
    }

    /// A header, which may be a collector's credential, never crosses the
    /// network in clear: beside an `http://` endpoint on another host it is
    /// refused naming its variable and the endpoint's setter, never a value.
    #[test]
    fn a_header_beside_a_plaintext_endpoint_on_another_host_is_refused() {
        for (variable, endpoint) in [
            (OTEL_EXPORTER_OTLP_HEADERS, "http://collector.internal:4318"),
            (OTEL_EXPORTER_OTLP_LOGS_HEADERS, "http://10.0.0.7:4318"),
            (OTEL_EXPORTER_OTLP_HEADERS, "http://[2001:db8::1]:4318"),
            (OTEL_EXPORTER_OTLP_HEADERS, "http://localhost.example:4318"),
        ] {
            let set: &'static [(&'static str, &'static str)] = match variable {
                OTEL_EXPORTER_OTLP_HEADERS => &[(OTEL_EXPORTER_OTLP_HEADERS, "api-key=s3cr3t")],
                _ => &[(OTEL_EXPORTER_OTLP_LOGS_HEADERS, "api-key=s3cr3t")],
            };
            let Err(refused) = resolved(Signal::Logs, Some(endpoint), set) else {
                panic!("a header over plaintext to {endpoint} must not build");
            };
            let said = refused.to_string();
            assert!(
                said.contains(variable) && said.contains(PINNED_ENDPOINT),
                "{said}"
            );
            assert!(
                !said.contains("s3cr3t") && !said.contains("api-key"),
                "{said}"
            );
        }
    }

    /// On this host's loopback the credential never leaves the machine, and
    /// over https it is encrypted: both export, a loopback collector dialled
    /// past any proxy.
    #[test]
    fn a_header_to_loopback_or_over_https_exports() {
        const HEADERS: &[(&str, &str)] = &[(OTEL_EXPORTER_OTLP_HEADERS, "api-key=s3cr3t")];
        for (endpoint, loopback) in [
            ("http://localhost:4318", true),
            ("http://LOCALHOST:4318", true),
            ("http://127.0.0.1:4318", true),
            ("http://127.3.2.1:4318", true),
            ("http://[::1]:4318", true),
            ("https://localhost:4318", true),
            ("https://collector.internal:4318", false),
        ] {
            let export = resolved(Signal::Logs, Some(endpoint), HEADERS)
                .expect(endpoint)
                .expect(endpoint);
            assert_eq!(export.loopback, loopback, "{endpoint}");
        }
    }

    /// An endpoint's own query, which every request line carries, may hold a
    /// collector's credential: in plaintext to another host it is refused
    /// naming the variable that set the endpoint, never quoting it.
    #[test]
    fn an_endpoint_carrying_a_query_in_plaintext_to_another_host_is_refused() {
        let Err(refused) = resolved(
            Signal::Logs,
            None,
            &[(
                OTEL_EXPORTER_OTLP_LOGS_ENDPOINT,
                "http://collector.internal:4318/v1/logs?api-key=s3cr3t",
            )],
        ) else {
            panic!("a query in clear must not build");
        };
        let said = refused.to_string();
        assert!(
            said.contains(OTEL_EXPORTER_OTLP_LOGS_ENDPOINT) && said.contains("carries a query"),
            "{said}"
        );
        assert!(
            !said.contains("s3cr3t") && !said.contains("api-key"),
            "{said}"
        );
    }

    /// A signal's own endpoint carrying a query exports over https or to this
    /// host's loopback, used as given; an empty query carries nothing.
    #[test]
    fn a_signal_s_own_endpoint_carrying_a_query_over_https_or_to_loopback_exports() {
        for endpoint in [
            "https://collector.internal:4318/v1/logs?api-key=s3cr3t",
            "http://localhost:4318/v1/logs?api-key=s3cr3t",
            "http://collector.internal:4318/v1/logs?",
        ] {
            let export = resolved(
                Signal::Logs,
                None,
                &[(OTEL_EXPORTER_OTLP_LOGS_ENDPOINT, endpoint)],
            )
            .expect(endpoint)
            .expect(endpoint);
            assert_eq!(export.url.as_str(), endpoint);
        }
    }

    /// Userinfo reaches no collector — the HTTP client builds its request
    /// without it — so in any endpoint, over https and to loopback alike, it
    /// is refused naming the variable or the field that set it and pointing
    /// to the headers, never quoting it.
    #[test]
    fn userinfo_in_any_endpoint_is_refused_naming_its_setter_and_pointing_to_the_headers() {
        for (set_by, endpoint) in [
            (
                OTEL_EXPORTER_OTLP_LOGS_ENDPOINT,
                "https://tenant:s3cr3t@collector.internal:4318/v1/logs",
            ),
            (
                OTEL_EXPORTER_OTLP_ENDPOINT,
                "http://tenant:s3cr3t@127.0.0.1:4318",
            ),
            (
                OTEL_EXPORTER_OTLP_ENDPOINT,
                "http://tenant:s3cr3t@collector.internal:4318",
            ),
            (PINNED_ENDPOINT, "https://s3cr3t@collector.internal:4318"),
            (PINNED_ENDPOINT, "http://:s3cr3t@localhost:4318"),
        ] {
            let (pinned, set) = if set_by == PINNED_ENDPOINT {
                (Some(endpoint), Vec::new())
            } else {
                (None, vec![(set_by, endpoint)])
            };
            let Err(refused) = resolved(Signal::Logs, pinned, &set) else {
                panic!("{set_by} carrying userinfo must not build");
            };
            let said = refused.to_string();
            assert!(
                said.contains(set_by)
                    && said.contains("userinfo")
                    && said.contains(OTEL_EXPORTER_OTLP_HEADERS)
                    && said.contains(OTEL_EXPORTER_OTLP_LOGS_HEADERS),
                "{said}"
            );
            assert!(
                !said.contains("s3cr3t") && !said.contains("tenant"),
                "{said}"
            );
        }
    }

    /// The specification sends each signal to a path relative to the base,
    /// which leaves a base's query no place: refused naming its setter and the
    /// signals' own endpoints, never quoted. A path on the base is kept, its
    /// trailing slash or not, and so is a fragment, which no request sends.
    #[test]
    fn a_base_endpoint_sets_each_signal_s_path_and_refuses_a_query() {
        for (pinned, set, set_by) in [
            (
                None,
                &[(
                    OTEL_EXPORTER_OTLP_ENDPOINT,
                    "https://collector:4318?tenant=s3cr3t",
                )][..],
                OTEL_EXPORTER_OTLP_ENDPOINT,
            ),
            (
                Some("https://collector:4318/otlp/?tenant=s3cr3t"),
                &[][..],
                PINNED_ENDPOINT,
            ),
        ] {
            let Err(refused) = resolved(Signal::Traces, pinned, set) else {
                panic!("{set_by} carrying a query must not build");
            };
            let said = refused.to_string();
            assert!(
                said.contains(set_by)
                    && said.contains("carries a query")
                    && said.contains(OTEL_EXPORTER_OTLP_TRACES_ENDPOINT)
                    && said.contains(OTEL_EXPORTER_OTLP_LOGS_ENDPOINT),
                "{said}"
            );
            assert!(
                !said.contains("s3cr3t") && !said.contains("tenant"),
                "{said}"
            );
        }
        for (base, traces) in [
            (
                "https://collector:4318/otlp",
                "https://collector:4318/otlp/v1/traces",
            ),
            (
                "https://collector:4318/otlp/",
                "https://collector:4318/otlp/v1/traces",
            ),
            (
                "https://collector:4318?",
                "https://collector:4318/v1/traces",
            ),
            (
                "https://collector:4318#here",
                "https://collector:4318/v1/traces#here",
            ),
        ] {
            let export = resolved(Signal::Traces, None, &[(OTEL_EXPORTER_OTLP_ENDPOINT, base)])
                .expect(base)
                .expect(base);
            assert_eq!(export.url.as_str(), traces, "{base}");
        }
    }

    /// Each processor's own bound and the exporter's cut one export, the
    /// earlier first: `OTEL_BSP_EXPORT_TIMEOUT` a span export,
    /// `OTEL_METRIC_EXPORT_TIMEOUT` a metric export, `OTEL_BLRP_EXPORT_TIMEOUT`
    /// a log export, each 30 s by default; one that is not a whole number of
    /// milliseconds is refused naming it.
    #[test]
    fn each_processor_s_bound_and_the_exporter_s_cut_an_export_the_earlier_first() {
        assert_eq!(export_timeouts(&[]), [10_000; 3]);
        assert_eq!(
            export_timeouts(&[(OTEL_METRIC_EXPORT_TIMEOUT, "2000")]),
            [10_000, 2000, 10_000]
        );
        assert_eq!(
            export_timeouts(&[
                (OTEL_BSP_EXPORT_TIMEOUT, "1500"),
                (OTEL_BLRP_EXPORT_TIMEOUT, "2500"),
            ]),
            [1500, 10_000, 2500]
        );
        assert_eq!(
            export_timeouts(&[
                (OTEL_METRIC_EXPORT_TIMEOUT, "60000"),
                (OTEL_EXPORTER_OTLP_TIMEOUT, "40000"),
            ]),
            [30_000, 40_000, 30_000]
        );
        assert_eq!(
            export_timeouts(&[
                (OTEL_METRIC_EXPORT_TIMEOUT, "20000"),
                (OTEL_EXPORTER_OTLP_METRICS_TIMEOUT, "45000"),
            ]),
            [10_000, 20_000, 10_000]
        );
        for (signal, variable) in [
            (Signal::Traces, OTEL_BSP_EXPORT_TIMEOUT),
            (Signal::Metrics, OTEL_METRIC_EXPORT_TIMEOUT),
            (Signal::Logs, OTEL_BLRP_EXPORT_TIMEOUT),
        ] {
            let Err(refused) = resolved(signal, Some("http://collector:4318"), &[(variable, "5s")])
            else {
                panic!("{variable} not in milliseconds must not build");
            };
            let said = refused.to_string();
            assert!(
                said.contains(variable) && said.contains(MILLISECONDS),
                "{said}"
            );
        }
    }

    /// A number a batch processor reads itself is refused where the SDK would
    /// pass it over — not a whole number, or padded, which the SDK does not
    /// trim — naming the variable, never quoting it; each processor's own,
    /// read where its signal exports.
    #[test]
    fn a_batch_processor_number_the_sdk_would_pass_over_is_refused_naming_it() {
        for (signal, variable, value) in [
            (Signal::Traces, OTEL_BSP_SCHEDULE_DELAY, "5s"),
            (Signal::Traces, OTEL_BSP_MAX_QUEUE_SIZE, " 2048"),
            (Signal::Traces, OTEL_BSP_MAX_EXPORT_BATCH_SIZE, "-1"),
            (Signal::Logs, OTEL_BLRP_SCHEDULE_DELAY, "1.5"),
            (Signal::Logs, OTEL_BLRP_MAX_QUEUE_SIZE, "4k"),
            (Signal::Logs, OTEL_BLRP_MAX_EXPORT_BATCH_SIZE, "lots"),
        ] {
            let Err(refused) =
                resolved(signal, Some("http://collector:4318"), &[(variable, value)])
            else {
                panic!("{variable}={value:?} must not build");
            };
            let said = refused.to_string();
            assert!(
                said.contains(variable) && said.contains("whole number"),
                "{said}"
            );
            assert!(value.trim() == "2048" || !said.contains(value), "{said}");
        }
        for (signal, variable) in [
            (Signal::Traces, OTEL_BSP_MAX_QUEUE_SIZE),
            (Signal::Logs, OTEL_BLRP_SCHEDULE_DELAY),
        ] {
            assert!(
                resolved(signal, Some("http://collector:4318"), &[(variable, "512")])
                    .expect("a whole number")
                    .is_some()
            );
        }
        assert!(
            resolved(
                Signal::Metrics,
                Some("http://collector:4318"),
                &[(OTEL_BSP_SCHEDULE_DELAY, "5s")]
            )
            .expect("the span processor's number is no metric export's")
            .is_some()
        );
    }

    /// A queue or batch size its batch processor cannot run with as written
    /// is refused naming it, never quoting it: none, which the specification
    /// calls invalid — a queue of none takes no span or record, every one
    /// dropped — and a batch written above its queue, which the SDK lowers to
    /// the queue in silence. A queue below the default batch with no batch
    /// written runs, the SDK lowering its own default.
    #[test]
    fn a_queue_or_batch_size_its_processor_cannot_run_with_is_refused_naming_it() {
        for (signal, set, variable, expected) in [
            (
                Signal::Traces,
                &[(OTEL_BSP_MAX_QUEUE_SIZE, "0")][..],
                OTEL_BSP_MAX_QUEUE_SIZE,
                "positive",
            ),
            (
                Signal::Traces,
                &[(OTEL_BSP_MAX_EXPORT_BATCH_SIZE, "0")][..],
                OTEL_BSP_MAX_EXPORT_BATCH_SIZE,
                "positive",
            ),
            (
                Signal::Logs,
                &[(OTEL_BLRP_MAX_QUEUE_SIZE, "0")][..],
                OTEL_BLRP_MAX_QUEUE_SIZE,
                "positive",
            ),
            (
                Signal::Logs,
                &[(OTEL_BLRP_MAX_EXPORT_BATCH_SIZE, "0")][..],
                OTEL_BLRP_MAX_EXPORT_BATCH_SIZE,
                "positive",
            ),
            (
                Signal::Traces,
                &[(OTEL_BSP_MAX_EXPORT_BATCH_SIZE, "4096")][..],
                OTEL_BSP_MAX_EXPORT_BATCH_SIZE,
                OTEL_BSP_MAX_QUEUE_SIZE,
            ),
            (
                Signal::Logs,
                &[
                    (OTEL_BLRP_MAX_QUEUE_SIZE, "100"),
                    (OTEL_BLRP_MAX_EXPORT_BATCH_SIZE, "101"),
                ][..],
                OTEL_BLRP_MAX_EXPORT_BATCH_SIZE,
                OTEL_BLRP_MAX_QUEUE_SIZE,
            ),
        ] {
            let Err(refused) = resolved(signal, Some("http://collector:4318"), set) else {
                panic!("{set:?} must not build");
            };
            let said = refused.to_string();
            assert!(said.contains(variable) && said.contains(expected), "{said}");
            assert!(
                !said.contains("4096") && !said.contains("100") && !said.contains("101"),
                "{said}"
            );
        }
        for (signal, set) in [
            (Signal::Traces, &[(OTEL_BSP_MAX_QUEUE_SIZE, "100")][..]),
            (
                Signal::Logs,
                &[
                    (OTEL_BLRP_MAX_QUEUE_SIZE, "1"),
                    (OTEL_BLRP_MAX_EXPORT_BATCH_SIZE, "1"),
                ][..],
            ),
        ] {
            assert!(
                resolved(signal, Some("http://collector:4318"), set)
                    .expect("a size its processor runs with")
                    .is_some(),
                "{set:?}"
            );
        }
    }

    /// A number its batch processor would turn into a spin or into an
    /// allocation no process holds is refused naming it, never quoting it,
    /// as the metric period is: a schedule delay under a millisecond, on which
    /// the SDK's worker polls its channel without pause, a core per processor
    /// idle or not; and a queue above the ceiling, every slot of which the SDK
    /// allocates when its processor starts, said with what the ceiling costs
    /// up front — the queue, and the batch buffer its worker allocates as it
    /// starts, as large as a batch that may reach the queue. Each at its bound
    /// runs.
    #[test]
    fn a_batch_number_the_sdk_would_spin_on_or_allocate_without_bound_is_refused_naming_it() {
        let above = (QUEUE_CEILING + 1).to_string();
        let huge = usize::MAX.to_string();
        let up_front = |queue_slot: usize, batch_slot: usize| {
            format!(
                "up to {} MiB at that ceiling",
                (QUEUE_CEILING * (queue_slot + batch_slot)).div_ceil(1 << 20)
            )
        };
        let spans = up_front(slot_bytes::<SpanData>(), size_of::<SpanData>());
        let records = up_front(
            slot_bytes::<QueuedLogRecord>(),
            size_of::<QueuedLogRecord>(),
        );
        for (signal, variable, value, expected) in [
            (
                Signal::Traces,
                OTEL_BSP_SCHEDULE_DELAY,
                "0",
                "without pause",
            ),
            (Signal::Logs, OTEL_BLRP_SCHEDULE_DELAY, "0", "without pause"),
            (
                Signal::Traces,
                OTEL_BSP_MAX_QUEUE_SIZE,
                above.as_str(),
                spans.as_str(),
            ),
            (
                Signal::Logs,
                OTEL_BLRP_MAX_QUEUE_SIZE,
                huge.as_str(),
                records.as_str(),
            ),
        ] {
            let Err(refused) =
                resolved(signal, Some("http://collector:4318"), &[(variable, value)])
            else {
                panic!("{variable}={value:?} must not build");
            };
            let said = refused.to_string();
            assert!(said.contains(variable) && said.contains(expected), "{said}");
            assert!(!said.contains(&huge) && !said.contains(&above), "{said}");
        }

        let ceiling = QUEUE_CEILING.to_string();
        for (signal, set) in [
            (Signal::Traces, [(OTEL_BSP_SCHEDULE_DELAY, "1")]),
            (Signal::Logs, [(OTEL_BLRP_SCHEDULE_DELAY, "1")]),
            (
                Signal::Traces,
                [(OTEL_BSP_MAX_QUEUE_SIZE, ceiling.as_str())],
            ),
            (Signal::Logs, [(OTEL_BLRP_MAX_QUEUE_SIZE, ceiling.as_str())]),
        ] {
            assert!(
                resolved(signal, Some("http://collector:4318"), &set)
                    .expect("a number its processor runs with")
                    .is_some(),
                "{set:?}"
            );
        }
    }

    /// A zero timeout is no limit, as the specification reads a timeout's
    /// zero, so the export's other bound cuts it: the processor's under the
    /// exporter's zero, the exporter's under the processor's; a signal's own
    /// zero is its own, never the general variable's value. Both at zero leave
    /// nothing bounding an export, and every wait the framework owns is
    /// bounded: refused naming both, never quoting them.
    #[test]
    fn a_zero_timeout_is_no_limit_and_the_export_s_other_bound_cuts_it() {
        assert_eq!(
            export_timeouts(&[(OTEL_EXPORTER_OTLP_TIMEOUT, "0")]),
            [30_000; 3]
        );
        assert_eq!(
            export_timeouts(&[
                (OTEL_BSP_EXPORT_TIMEOUT, "0"),
                (OTEL_METRIC_EXPORT_TIMEOUT, "0"),
                (OTEL_BLRP_EXPORT_TIMEOUT, "0"),
            ]),
            [10_000; 3]
        );
        assert_eq!(
            export_timeouts(&[
                (OTEL_EXPORTER_OTLP_TIMEOUT, "2000"),
                (OTEL_EXPORTER_OTLP_LOGS_TIMEOUT, "0"),
            ]),
            [2000, 2000, 30_000]
        );
        for (signal, exporter, processor) in [
            (
                Signal::Traces,
                OTEL_EXPORTER_OTLP_TIMEOUT,
                OTEL_BSP_EXPORT_TIMEOUT,
            ),
            (
                Signal::Metrics,
                OTEL_EXPORTER_OTLP_METRICS_TIMEOUT,
                OTEL_METRIC_EXPORT_TIMEOUT,
            ),
            (
                Signal::Logs,
                OTEL_EXPORTER_OTLP_TIMEOUT,
                OTEL_BLRP_EXPORT_TIMEOUT,
            ),
        ] {
            let Err(refused) = resolved(
                signal,
                Some("http://collector:4318"),
                &[(exporter, "0"), (processor, "0")],
            ) else {
                panic!("{exporter} and {processor} at zero must not build");
            };
            let said = refused.to_string();
            assert!(
                said.contains(exporter) && said.contains(processor) && said.contains("no limit"),
                "{said}"
            );
        }
    }

    /// A service name that names nothing is refused wherever it is written,
    /// naming where and never quoting it: `OTEL_SERVICE_NAME` blank, which the
    /// SDK's detector would take as the name over the binary's, were it read
    /// as unset here; a `service.name` in `OTEL_RESOURCE_ATTRIBUTES` empty or
    /// blank once decoded; a pinned `service_name` empty or blank. Empty,
    /// `OTEL_SERVICE_NAME` is unset, as the specification orders, and the
    /// binary names the service; padded, it is the name as written, as the
    /// SDK reads it too.
    #[test]
    fn a_service_name_that_names_nothing_is_refused_wherever_it_is_written() {
        let built = |set: &[(&str, &str)], pinned: Option<&str>| -> Result<String, String> {
            let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let config = OpenTelemetryConfig {
                service_name: pinned.map(str::to_owned),
                ..OpenTelemetryConfig::default()
            };
            let mut outcome = None;
            figment::Jail::expect_with(|jail| {
                for (variable, value) in set {
                    jail.set_env(variable, value);
                }
                outcome = Some(
                    match build_reading(&config, Some(&binary()), nest_rs_config::env_var) {
                        Ok(exporters) => {
                            let service = exporters.summary.service.clone();
                            shut(exporters);
                            Ok(service)
                        }
                        Err(refused) => Err(refused.to_string()),
                    },
                );
                Ok(())
            });
            outcome.expect("the jail ran")
        };
        for (set, pinned, named) in [
            (&[(OTEL_SERVICE_NAME, "  ")][..], None, OTEL_SERVICE_NAME),
            (&[(OTEL_SERVICE_NAME, "\t")][..], None, OTEL_SERVICE_NAME),
            (
                &[(OTEL_RESOURCE_ATTRIBUTES, "service.name=")][..],
                None,
                OTEL_RESOURCE_ATTRIBUTES,
            ),
            (
                &[(OTEL_RESOURCE_ATTRIBUTES, "team=a, service.name = ")][..],
                None,
                OTEL_RESOURCE_ATTRIBUTES,
            ),
            (
                &[(OTEL_RESOURCE_ATTRIBUTES, "service.name=%20%09")][..],
                None,
                OTEL_RESOURCE_ATTRIBUTES,
            ),
            (&[][..], Some(""), PINNED_SERVICE_NAME),
            (&[][..], Some("   "), PINNED_SERVICE_NAME),
        ] {
            let refused = match built(set, pinned) {
                Ok(service) => panic!("{set:?} {pinned:?} must not build, named {service:?}"),
                Err(refused) => refused,
            };
            assert!(
                refused.contains(named) && refused.contains("names no service"),
                "{refused}"
            );
            assert!(!refused.contains("team"), "{refused}");
        }
        assert_eq!(
            built(&[(OTEL_SERVICE_NAME, "")], None).as_deref(),
            Ok("auth")
        );
        assert_eq!(
            built(&[(OTEL_SERVICE_NAME, " billing ")], None).as_deref(),
            Ok(" billing ")
        );
    }

    /// A signal's own headers, then the general ones; a blank signal variable
    /// is unset, as the specification orders, so the general headers reach
    /// it. Each pair parsed as the SDK parses it — trimmed, an empty member
    /// skipped — and each value kept as written, trimmed where it decodes,
    /// for the exporter to decode once.
    #[test]
    fn a_signal_s_headers_are_its_own_then_the_general_ones_and_blank_is_unset() {
        let read = |set: &[(&str, &str)], signal: Signal| {
            headers(signal, &blank_as_unset(reading(set))).expect("readable")
        };
        let pairs = |set_by: &'static str, pairs: &[(&str, &str)]| {
            Some(Headers {
                set_by,
                pairs: pairs
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), Secret::from(*value)))
                    .collect(),
            })
        };
        assert_eq!(read(&[], Signal::Logs), None);
        let set = [
            (
                OTEL_EXPORTER_OTLP_HEADERS,
                " api-key = s3cr3t , x-tenant= a%2520b ,, ",
            ),
            (OTEL_EXPORTER_OTLP_TRACES_HEADERS, "x-probe=traces"),
            (OTEL_EXPORTER_OTLP_LOGS_HEADERS, "  "),
        ];
        assert_eq!(
            read(&set, Signal::Logs),
            pairs(
                OTEL_EXPORTER_OTLP_HEADERS,
                &[("api-key", "s3cr3t"), ("x-tenant", "a%2520b")]
            ),
            "a blank signal variable is unset, and a value is handed undecoded"
        );
        assert_eq!(
            read(&set, Signal::Traces),
            pairs(OTEL_EXPORTER_OTLP_TRACES_HEADERS, &[("x-probe", "traces")]),
            "a signal's own wins over the general ones"
        );
        assert_eq!(
            read(
                &[(OTEL_EXPORTER_OTLP_HEADERS, "x-raw= 100%zz")],
                Signal::Metrics
            ),
            pairs(OTEL_EXPORTER_OTLP_HEADERS, &[("x-raw", " 100%zz")]),
            "an escape that does not decode is sent as written, untrimmed, as the SDK sends it"
        );
        assert_eq!(
            read(&[(OTEL_EXPORTER_OTLP_HEADERS, " , ")], Signal::Logs),
            None
        );
    }

    /// A pair the SDK would drop in silence — no `=`, an empty key or value, a
    /// key no header may carry, a value no header may hold — is refused naming
    /// the variable, never quoting the pair.
    #[test]
    fn a_header_pair_the_sdk_would_drop_is_refused_naming_its_variable() {
        for value in [
            "s3cr3t",
            "api-key=s3cr3t,=s3cr3t",
            "api-key=",
            "api key=s3cr3t",
            "api-key=%0As3cr3t",
        ] {
            let Err(refused) = headers(
                Signal::Metrics,
                &reading(&[(OTEL_EXPORTER_OTLP_METRICS_HEADERS, value)]),
            ) else {
                panic!("{value:?} must not build");
            };
            let said = refused.to_string();
            assert!(
                said.contains(OTEL_EXPORTER_OTLP_METRICS_HEADERS) && said.contains("key=value"),
                "{said}"
            );
            assert!(!said.contains("s3cr3t"), "{said}");
        }
    }

    /// `percent_decoded` decodes as opentelemetry-otlp 0.33's `url_decode`: a
    /// run of escapes at a time, `None` on a malformed escape or a run that is
    /// not UTF-8.
    #[test]
    fn percent_decoding_matches_the_sdk_s() {
        for (value, decoded) in [
            ("plain", Some("plain")),
            ("a%20b", Some("a b")),
            ("caf%C3%A9", Some("caf\u{e9}")),
            ("%", None),
            ("%z", None),
            ("%zz", None),
            ("%C3", None),
            ("%C3x%A9", None),
        ] {
            assert_eq!(percent_decoded(value).as_deref(), decoded, "{value:?}");
        }
    }

    /// The span limits: each its own variable over the SDK's default, a
    /// span's, an event's and a link's attribute count falling back on
    /// `OTEL_ATTRIBUTE_COUNT_LIMIT`; one that is not a whole number is
    /// refused naming it, never quoted.
    #[test]
    fn the_span_limits_are_each_its_own_variable_then_the_general_attribute_count() {
        let limits = |set: &[(&str, &str)]| {
            let limits = span_limits(&reading(set)).expect("whole numbers");
            [
                limits.max_attributes_per_span,
                limits.max_events_per_span,
                limits.max_links_per_span,
                limits.max_attributes_per_event,
                limits.max_attributes_per_link,
            ]
        };
        assert_eq!(limits(&[]), [128; 5], "the SDK's defaults");
        assert_eq!(
            limits(&[(OTEL_ATTRIBUTE_COUNT_LIMIT, "64")]),
            [64, 128, 128, 64, 64]
        );
        assert_eq!(
            limits(&[
                (OTEL_ATTRIBUTE_COUNT_LIMIT, "64"),
                (OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT, "32"),
                (OTEL_SPAN_EVENT_COUNT_LIMIT, "8"),
                (OTEL_SPAN_LINK_COUNT_LIMIT, "4"),
                (OTEL_EVENT_ATTRIBUTE_COUNT_LIMIT, "2"),
                (OTEL_LINK_ATTRIBUTE_COUNT_LIMIT, " 1 "),
            ]),
            [32, 8, 4, 2, 1]
        );
        for variable in [
            OTEL_ATTRIBUTE_COUNT_LIMIT,
            OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT,
            OTEL_SPAN_EVENT_COUNT_LIMIT,
            OTEL_SPAN_LINK_COUNT_LIMIT,
            OTEL_EVENT_ATTRIBUTE_COUNT_LIMIT,
            OTEL_LINK_ATTRIBUTE_COUNT_LIMIT,
        ] {
            let said = span_limits(&reading(&[(variable, "-5")]))
                .expect_err("not a whole number")
                .to_string();
            assert!(
                said.contains(variable) && said.contains("whole number"),
                "{said}"
            );
            assert!(!said.contains("-5"), "{said}");
        }
    }

    /// A number past what its variable's type holds is a whole number still:
    /// a span limit, a millisecond setting and a batch processor's number are
    /// refused saying the bound, never quoting the value.
    #[test]
    fn a_number_past_its_variable_s_bound_is_refused_saying_the_bound() {
        let past_u32 = (u64::from(u32::MAX) + 1).to_string();
        let past_u64 = (u128::from(u64::MAX) + 1).to_string();
        let past_usize = (u128::try_from(usize::MAX).expect("a usize fits") + 1).to_string();
        for (variable, said, bound, past) in [
            (
                OTEL_SPAN_EVENT_COUNT_LIMIT,
                span_limits(&reading(&[(OTEL_SPAN_EVENT_COUNT_LIMIT, &past_u32)])).map(drop),
                u32::MAX.to_string(),
                &past_u32,
            ),
            (
                OTEL_EXPORTER_OTLP_TIMEOUT,
                milliseconds(
                    OTEL_EXPORTER_OTLP_TIMEOUT,
                    &reading(&[(OTEL_EXPORTER_OTLP_TIMEOUT, &past_u64)]),
                )
                .map(drop),
                u64::MAX.to_string(),
                &past_u64,
            ),
            (
                OTEL_BSP_SCHEDULE_DELAY,
                refuse_unservable_batch_numbers(
                    Signal::Traces,
                    &reading(&[(OTEL_BSP_SCHEDULE_DELAY, &past_u64)]),
                ),
                u64::MAX.to_string(),
                &past_u64,
            ),
            (
                OTEL_BLRP_MAX_QUEUE_SIZE,
                refuse_unservable_batch_numbers(
                    Signal::Logs,
                    &reading(&[(OTEL_BLRP_MAX_QUEUE_SIZE, &past_usize)]),
                ),
                usize::MAX.to_string(),
                &past_usize,
            ),
        ] {
            let said = said.expect_err(variable).to_string();
            assert!(
                said.contains(variable) && said.contains(&format!("up to {bound}")),
                "{said}"
            );
            assert!(!said.contains(past.as_str()), "{said}");
        }
    }

    /// The span the tracer every build starts from records under `set`'s span
    /// limits: three attributes on it and on its one event.
    fn recorded_under(set: &[(&str, &str)]) -> opentelemetry_sdk::trace::SpanData {
        use opentelemetry::trace::{Span as _, Tracer as _};
        use opentelemetry_sdk::trace::InMemorySpanExporter;

        let exporter = InMemorySpanExporter::default();
        let provider = tracer_provider(
            Resource::builder_empty().build(),
            Sampler::AlwaysOn,
            span_limits(&reading(set)).expect("a count"),
        )
        .with_simple_exporter(exporter.clone())
        .build();
        let attributes = || -> Vec<KeyValue> {
            (0_i64..3)
                .map(|n| KeyValue::new(format!("key.{n}"), n))
                .collect()
        };
        let tracer = provider.tracer("limits");
        let mut span = tracer
            .span_builder("bounded")
            .with_attributes(attributes())
            .start(&tracer);
        span.add_event("event", attributes());
        span.end();
        let recorded = exporter
            .get_finished_spans()
            .expect("the spans exported")
            .pop()
            .expect("one span");
        let _ = provider.shutdown();
        recorded
    }

    /// The tracer every build starts from records within the span limits
    /// resolved: two attributes on a span and on its event under
    /// `OTEL_ATTRIBUTE_COUNT_LIMIT=2`, the rest dropped and counted.
    #[test]
    fn the_tracer_records_within_the_span_limits_resolved() {
        let span = recorded_under(&[(OTEL_ATTRIBUTE_COUNT_LIMIT, "2")]);
        assert_eq!(span.attributes.len(), 2, "{:?}", span.attributes);
        assert_eq!(span.dropped_attributes_count, 1);
        let event = span.events.events.first().expect("one event");
        assert_eq!(event.attributes.len(), 2, "{event:?}");
    }

    /// A zero count limit is a limit, as the specification's non-negative
    /// range reads it: the span keeps no attribute and no event, each dropped
    /// and counted.
    #[test]
    fn a_zero_count_limit_records_none() {
        let span = recorded_under(&[
            (OTEL_ATTRIBUTE_COUNT_LIMIT, "0"),
            (OTEL_SPAN_EVENT_COUNT_LIMIT, "0"),
        ]);
        assert!(span.attributes.is_empty(), "{:?}", span.attributes);
        assert_eq!(span.dropped_attributes_count, 3);
        assert!(span.events.events.is_empty(), "{:?}", span.events);
        assert_eq!(span.events.dropped_count, 1);
    }

    /// A specification variable this build serves nowhere joins
    /// `passed_over`, each once and with why, unless its value names what the
    /// build does — a line on correct configuration teaches an operator to
    /// filter the target. The general attribute count is one only where the
    /// logs export and no log record's own limit is set.
    #[test]
    fn a_variable_this_build_serves_nowhere_is_passed_over_unless_its_value_is_what_it_does() {
        let said = |set: &[(&str, &str)], logs_export: bool| -> Vec<&'static str> {
            unserved(&reading(set), logs_export)
                .iter()
                .map(|over| over.variable)
                .collect()
        };
        assert!(said(&[], true).is_empty());
        assert_eq!(
            said(
                &[
                    (OTEL_LOG_LEVEL, "info"),
                    (OTEL_ATTRIBUTE_VALUE_LENGTH_LIMIT, "4095"),
                    (OTEL_SPAN_ATTRIBUTE_VALUE_LENGTH_LIMIT, "4095"),
                    (OTEL_LOGRECORD_ATTRIBUTE_VALUE_LENGTH_LIMIT, "4095"),
                    (OTEL_LOGRECORD_ATTRIBUTE_COUNT_LIMIT, "64"),
                    (OTEL_METRICS_EXEMPLAR_FILTER, "trace_based"),
                    (
                        OTEL_EXPORTER_OTLP_METRICS_DEFAULT_HISTOGRAM_AGGREGATION,
                        "base2_exponential_bucket_histogram"
                    ),
                    (OTEL_BSP_MAX_CONCURRENT_EXPORTS, "4"),
                    (OTEL_EXPORTER_OTLP_INSECURE, "true"),
                    (OTEL_EXPORTER_OTLP_TRACES_INSECURE, "true"),
                    (OTEL_EXPORTER_OTLP_METRICS_INSECURE, "TRUE"),
                    (OTEL_EXPORTER_OTLP_LOGS_INSECURE, "anything"),
                ],
                true
            ),
            [
                OTEL_ATTRIBUTE_VALUE_LENGTH_LIMIT,
                OTEL_SPAN_ATTRIBUTE_VALUE_LENGTH_LIMIT,
                OTEL_LOGRECORD_ATTRIBUTE_VALUE_LENGTH_LIMIT,
                OTEL_LOGRECORD_ATTRIBUTE_COUNT_LIMIT,
                OTEL_METRICS_EXEMPLAR_FILTER,
                OTEL_EXPORTER_OTLP_METRICS_DEFAULT_HISTOGRAM_AGGREGATION,
                OTEL_LOG_LEVEL,
                OTEL_BSP_MAX_CONCURRENT_EXPORTS,
                OTEL_EXPORTER_OTLP_INSECURE,
                OTEL_EXPORTER_OTLP_TRACES_INSECURE,
                OTEL_EXPORTER_OTLP_METRICS_INSECURE,
                OTEL_EXPORTER_OTLP_LOGS_INSECURE,
            ]
        );
        assert!(
            said(
                &[
                    (OTEL_METRICS_EXEMPLAR_FILTER, " Always_Off "),
                    (
                        OTEL_EXPORTER_OTLP_METRICS_DEFAULT_HISTOGRAM_AGGREGATION,
                        "EXPLICIT_BUCKET_HISTOGRAM"
                    ),
                    (OTEL_BSP_MAX_CONCURRENT_EXPORTS, "1"),
                    (OTEL_EXPORTER_OTLP_INSECURE, "false"),
                    (OTEL_EXPORTER_OTLP_TRACES_INSECURE, " FALSE "),
                    (OTEL_EXPORTER_OTLP_METRICS_INSECURE, "False"),
                    (OTEL_EXPORTER_OTLP_LOGS_INSECURE, "false"),
                ],
                true
            )
            .is_empty(),
            "a value naming what the build does is honoured in silence"
        );
        assert_eq!(
            said(&[(OTEL_ATTRIBUTE_COUNT_LIMIT, "64")], true),
            [OTEL_ATTRIBUTE_COUNT_LIMIT]
        );
        assert!(said(&[(OTEL_ATTRIBUTE_COUNT_LIMIT, "64")], false).is_empty());
        assert_eq!(
            said(
                &[
                    (OTEL_ATTRIBUTE_COUNT_LIMIT, "64"),
                    (OTEL_LOGRECORD_ATTRIBUTE_COUNT_LIMIT, "64"),
                ],
                true
            ),
            [OTEL_LOGRECORD_ATTRIBUTE_COUNT_LIMIT]
        );
        for over in unserved(&reading(&[(OTEL_LOG_LEVEL, "warn")]), false) {
            assert!(!over.reason.contains("warn"), "{over:?}");
        }
        for over in unserved(&reading(&[(OTEL_EXPORTER_OTLP_INSECURE, "true")]), false) {
            assert_eq!(over.reason, GRPC_ALONE);
        }
    }

    /// The configuration file and the propagators are read before anything is
    /// built: either refused fails the build.
    #[test]
    fn a_configuration_file_or_a_foreign_propagator_fails_the_build() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for (variable, value) in [
            ("OTEL_CONFIG_FILE", "/etc/otel.yaml"),
            ("OTEL_PROPAGATORS", "b3"),
        ] {
            let read = reading(&[(variable, value)]);
            let Err(refused) = build_reading(&OpenTelemetryConfig::default(), None, read) else {
                panic!("{variable} must not build");
            };
            assert!(refused.to_string().contains(variable), "{refused}");
        }
    }

    /// A `PeriodicReader` on a zero period is a tight export loop; an hourly
    /// one holds an hour of state.
    #[test]
    fn a_metric_interval_outside_a_second_to_an_hour_is_refused_naming_what_set_it() {
        assert_eq!(
            bounded_interval(Duration::from_secs(5), "X").expect("inside the range"),
            Duration::from_secs(5)
        );
        for (interval, sentence) in [
            (Duration::ZERO, "below the 1s it must be at least"),
            (
                Duration::from_millis(999),
                "below the 1s it must be at least",
            ),
            (
                Duration::from_secs(3601),
                "above the 3600s it must be at most",
            ),
        ] {
            let refused = bounded_interval(interval, "OTEL_METRIC_EXPORT_INTERVAL")
                .expect_err("outside the range")
                .to_string();
            assert!(
                refused.contains("OTEL_METRIC_EXPORT_INTERVAL") && refused.contains(sentence),
                "{refused}"
            );
        }
    }
}
