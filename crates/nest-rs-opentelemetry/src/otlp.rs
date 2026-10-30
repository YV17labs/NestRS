//! OTel SDK wiring: a local tracer and W3C propagator always, and a batch OTLP
//! exporter over HTTP/protobuf for each signal — traces, metrics, logs — whose
//! endpoint is set.
//!
//! **The OpenTelemetry specification's variables configure it** (`OTEL_*`),
//! over what the app pinned in [`OpenTelemetryConfig`]: the SDK reads the
//! resource's and the sampler's itself, and what it read is not overridden;
//! the endpoints, the export timeouts, the metric period and the certificates
//! are resolved here as the specification orders them — a signal's own
//! variable, then the general one — through the SDK's constants where it
//! exports them, so a value is refused naming its variable, never quoted.
//!
//! **The collector is not proved at boot** (`nest_rs_core::prove`): telemetry
//! must never stop the app it observes, so an unreachable collector costs
//! exports, never the boot.

use std::time::Duration;

use nest_rs_config::{
    ClientTls, ConfigError, Material, TlsIdentity, TlsNames, TlsSetting, endpoint_shown,
};
use nest_rs_core::AppIdentity;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{Key, KeyValue, global};
use opentelemetry_otlp::{
    LogExporter, MetricExporter, OTEL_EXPORTER_OTLP_ENDPOINT, OTEL_EXPORTER_OTLP_LOGS_ENDPOINT,
    OTEL_EXPORTER_OTLP_LOGS_PROTOCOL, OTEL_EXPORTER_OTLP_LOGS_TIMEOUT,
    OTEL_EXPORTER_OTLP_METRICS_ENDPOINT, OTEL_EXPORTER_OTLP_METRICS_PROTOCOL,
    OTEL_EXPORTER_OTLP_METRICS_TIMEOUT, OTEL_EXPORTER_OTLP_PROTOCOL,
    OTEL_EXPORTER_OTLP_PROTOCOL_HTTP_PROTOBUF, OTEL_EXPORTER_OTLP_TIMEOUT,
    OTEL_EXPORTER_OTLP_TIMEOUT_DEFAULT, OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
    OTEL_EXPORTER_OTLP_TRACES_PROTOCOL, OTEL_EXPORTER_OTLP_TRACES_TIMEOUT, Protocol, SpanExporter,
    WithExportConfig, WithHttpConfig,
};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{OTEL_METRIC_EXPORT_INTERVAL, PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use opentelemetry_semantic_conventions::{
    SCHEMA_URL,
    attribute::{DEPLOYMENT_ENVIRONMENT_NAME, SERVICE_INSTANCE_ID, SERVICE_NAME, SERVICE_VERSION},
};
use uuid::Uuid;

use crate::config::OpenTelemetryConfig;
use crate::error::OpenTelemetryError;

// opentelemetry-otlp 0.33 quotes its request's URL, whose userinfo or query may
// carry a collector's credential, and the collector's error body at `debug`.
nest_rs_core::inventory::submit! {
    nest_rs_core::__private::LibraryClamp::new(
        "opentelemetry-otlp",
        tracing::level_filters::LevelFilter::INFO,
    )
}

// The specification's names for what the SDK reads itself, or not at all over
// HTTP; opentelemetry_sdk 0.33 keeps its own private.
const OTEL_SERVICE_NAME: &str = "OTEL_SERVICE_NAME";
const OTEL_TRACES_SAMPLER: &str = "OTEL_TRACES_SAMPLER";
const OTEL_EXPORTER_OTLP_CERTIFICATE: &str = "OTEL_EXPORTER_OTLP_CERTIFICATE";
const OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE: &str = "OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE";
const OTEL_EXPORTER_OTLP_CLIENT_KEY: &str = "OTEL_EXPORTER_OTLP_CLIENT_KEY";

/// The fields a config built in code sets, as a refusal names them.
const PINNED_ENDPOINT: &str = "`OpenTelemetryConfig::otlp_endpoint` in code";
const PINNED_TLS: &str = "`OpenTelemetryConfig::tls` in code";
const PINNED_INTERVAL: &str = "`OpenTelemetryConfig::metric_interval` in code";
const PINNED_RATIO: &str = "`OpenTelemetryConfig::trace_sample_ratio` in code";

/// One signal OTLP exports, and the variables the specification names for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Signal {
    Traces,
    Metrics,
    Logs,
}

impl Signal {
    fn name(self) -> &'static str {
        match self {
            Self::Traces => "traces",
            Self::Metrics => "metrics",
            Self::Logs => "logs",
        }
    }

    /// Its endpoint, used as given.
    fn endpoint_var(self) -> &'static str {
        match self {
            Self::Traces => OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
            Self::Metrics => OTEL_EXPORTER_OTLP_METRICS_ENDPOINT,
            Self::Logs => OTEL_EXPORTER_OTLP_LOGS_ENDPOINT,
        }
    }

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
    /// `Some` only when the metrics have an endpoint.
    pub meter_provider: Option<SdkMeterProvider>,
    /// `Some` only when the logs have an endpoint.
    pub logger_provider: Option<SdkLoggerProvider>,
    /// What the console's line about the exporters says.
    pub summary: Summary,
}

/// What the subscriber, once installed, says about what was built.
pub(crate) struct Summary {
    /// The `service.name` the resource carries.
    pub service: String,
    /// Each signal's endpoint, shown without its credential, or `<none>`.
    pub endpoints: [String; 3],
    /// Whether `OTEL_TRACES_SAMPLER` chose the sampler.
    pub sampler_from_env: bool,
    /// The variables passed over for not holding a whole number of
    /// milliseconds, said once a subscriber can hear it.
    pub ignored: Vec<&'static str>,
}

pub(crate) fn build(config: &OpenTelemetryConfig) -> Result<Exporters, OpenTelemetryError> {
    build_reading(config, AppIdentity::current(), nest_rs_config::env_var)
}

/// [`build`], with the binary's identity and the variables read through `read`.
fn build_reading(
    config: &OpenTelemetryConfig,
    binary: Option<&AppIdentity>,
    read: impl Fn(&str) -> Option<String>,
) -> Result<Exporters, OpenTelemetryError> {
    let read = |name: &str| read(name).filter(|value| !value.trim().is_empty());
    let mut ignored = Vec::new();
    let sampler = sampler(config, &read)?;
    let interval = metric_interval(config, &read, &mut ignored)?;
    let exports = [
        export(Signal::Traces, config, &read, &mut ignored)?,
        export(Signal::Metrics, config, &read, &mut ignored)?,
        export(Signal::Logs, config, &read, &mut ignored)?,
    ];
    let [traces_client, metrics_client, logs_client] = exporter_clients(&exports)?;
    let endpoints = exports.each_ref().map(|export| {
        export.as_ref().map_or_else(
            || "<none>".to_owned(),
            |export| endpoint_shown(export.url.as_str()),
        )
    });
    let [traces, metrics, logs] = exports;

    global::set_text_map_propagator(TraceContextPropagator::new());
    let resource = build_resource(config, binary, &read);
    let service = resource
        .get(&Key::new(SERVICE_NAME))
        .map(|name| name.to_string())
        .unwrap_or_default();

    let mut tracer_builder = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        // The SDK adopts the framework's ids rather than minting a second pair; see
        // `id_generator`.
        .with_id_generator(crate::id_generator::AdoptFrameworkIds::default());
    let sampler_from_env = sampler.is_none();
    if let Some(sampler) = sampler {
        tracer_builder = tracer_builder.with_sampler(sampler);
    }
    if let (Some(export), Some(client)) = (&traces, traces_client) {
        let span_exporter = SpanExporter::builder()
            .with_http()
            .with_http_client(client)
            .with_endpoint(export.url.as_str())
            .with_protocol(Protocol::HttpBinary)
            .build()
            .map_err(not_built(Signal::Traces, export))?;
        tracer_builder = tracer_builder.with_batch_exporter(span_exporter);
    }
    let tracer_provider = tracer_builder.build();
    let tracer = tracer_provider.tracer(service.clone());
    global::set_tracer_provider(tracer_provider.clone());

    let meter_provider = match (&metrics, metrics_client) {
        (Some(export), Some(client)) => {
            let metric_exporter = MetricExporter::builder()
                .with_http()
                .with_http_client(client)
                .with_endpoint(export.url.as_str())
                .with_protocol(Protocol::HttpBinary)
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
            let log_exporter = LogExporter::builder()
                .with_http()
                .with_http_client(client)
                .with_endpoint(export.url.as_str())
                .with_protocol(Protocol::HttpBinary)
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
            endpoints,
            sampler_from_env,
            ignored,
        },
    })
}

/// The sampler the app pinned, or `None` when `OTEL_TRACES_SAMPLER` names one,
/// which the SDK read. A pinned ratio outside `[0, 1]` is refused naming its
/// field, whichever applies.
fn sampler(
    config: &OpenTelemetryConfig,
    read: &impl Fn(&str) -> Option<String>,
) -> Result<Option<Sampler>, ConfigError> {
    let ratio = config.trace_sample_ratio;
    if !(0.0..=1.0).contains(&ratio) {
        return Err(ConfigError::parse(
            PINNED_RATIO,
            "must be a number between 0 and 1",
        ));
    }
    if read(OTEL_TRACES_SAMPLER).is_some() {
        return Ok(None);
    }
    Ok(Some(Sampler::ParentBased(Box::new(
        Sampler::TraceIdRatioBased(ratio),
    ))))
}

/// The metric export period: `OTEL_METRIC_EXPORT_INTERVAL`, in milliseconds,
/// over the one pinned, each held to a second to an hour. A value that is not
/// a whole number of milliseconds is passed over, as the SDK passes it over,
/// and joins `ignored`.
fn metric_interval(
    config: &OpenTelemetryConfig,
    read: &impl Fn(&str) -> Option<String>,
    ignored: &mut Vec<&'static str>,
) -> Result<Duration, ConfigError> {
    if let Some(raw) = read(OTEL_METRIC_EXPORT_INTERVAL) {
        match raw.trim().parse::<u64>() {
            Ok(millis) => {
                return bounded_interval(
                    Duration::from_millis(millis),
                    OTEL_METRIC_EXPORT_INTERVAL,
                );
            }
            Err(_) => ignored.push(OTEL_METRIC_EXPORT_INTERVAL),
        }
    }
    bounded_interval(config.metric_interval, PINNED_INTERVAL)
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

/// Where one signal exports, and what it exports through.
struct Export {
    url: url::Url,
    /// The variable, or the field in code, that set the endpoint.
    set_by: &'static str,
    plaintext: bool,
    timeout: Duration,
    tls: ClientTls,
    names: SignalTls,
}

/// Where `signal` exports — its own endpoint variable, used as given; then
/// `OTEL_EXPORTER_OTLP_ENDPOINT`, then the pinned endpoint, each with the
/// signal's path appended — or `None` when nothing sets one. A protocol
/// variable naming another than http/protobuf is refused rather than
/// overridden: the collector's port speaks what it names.
fn export(
    signal: Signal,
    config: &OpenTelemetryConfig,
    read: &impl Fn(&str) -> Option<String>,
    ignored: &mut Vec<&'static str>,
) -> Result<Option<Export>, ConfigError> {
    let (raw, set_by) = if let Some(full) = read(signal.endpoint_var()) {
        (full.trim().to_owned(), signal.endpoint_var())
    } else if let Some(base) = read(OTEL_EXPORTER_OTLP_ENDPOINT) {
        (with_path(&base, signal), OTEL_EXPORTER_OTLP_ENDPOINT)
    } else if let Some(base) = config.otlp_endpoint.as_deref() {
        (with_path(base, signal), PINNED_ENDPOINT)
    } else {
        return Ok(None);
    };
    let url = url::Url::parse(&raw)
        .map_err(|error| ConfigError::parse(set_by, format!("does not parse as a URL: {error}")))?;
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
    let (tls, names) = signal_tls(signal, &config.tls, read)?;
    if plaintext {
        tls.refuse_beside_plaintext(&names, set_by)?;
    }
    Ok(Some(Export {
        url,
        set_by,
        plaintext,
        timeout: export_timeout(signal.timeout_var(), read, ignored),
        tls,
        names,
    }))
}

/// `base` with `signal`'s path, one slash between.
fn with_path(base: &str, signal: Signal) -> String {
    format!("{}{}", base.trim().trim_end_matches('/'), signal.path())
}

/// How long one export of a signal may take: its `signal` variable, then
/// `OTEL_EXPORTER_OTLP_TIMEOUT`, in milliseconds, then the SDK's 10 s.
///
/// That is the SDK's own resolution, which bounds its retries, so an attempt is
/// cut where the SDK stops waiting for it. A value that is not a whole number of
/// milliseconds is passed over, as the SDK and the OpenTelemetry specification
/// pass it over, and its variable joins `ignored`.
fn export_timeout(
    signal: &'static str,
    read: &impl Fn(&str) -> Option<String>,
    ignored: &mut Vec<&'static str>,
) -> Duration {
    for variable in [signal, OTEL_EXPORTER_OTLP_TIMEOUT] {
        let Some(raw) = read(variable) else {
            continue;
        };
        match raw.trim().parse::<u64>() {
            Ok(millis) => return Duration::from_millis(millis),
            Err(_) if !ignored.contains(&variable) => ignored.push(variable),
            Err(_) => {}
        }
    }
    OTEL_EXPORTER_OTLP_TIMEOUT_DEFAULT
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
/// its signal's [`export_timeout`], and `https` alone unless the endpoint
/// declares plaintext with `http://` — the OTLP specification lets the
/// endpoint's scheme decide, and a collector beside the app is the common case.
///
/// Built on a thread of their own, as the SDK builds its own: a blocking client
/// built on a runtime's thread panics.
fn exporter_clients(
    exports: &[Option<Export>; 3],
) -> Result<[Option<reqwest::blocking::Client>; 3], OpenTelemetryError> {
    struct Plan {
        plaintext: bool,
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
/// name and version, under what the SDK read from `OTEL_RESOURCE_ATTRIBUTES`,
/// under `OTEL_SERVICE_NAME`, which wins.
fn build_resource(
    config: &OpenTelemetryConfig,
    binary: Option<&AppIdentity>,
    read: &impl Fn(&str) -> Option<String>,
) -> Resource {
    let detected = Resource::builder().build();
    let mut ours = vec![KeyValue::new(
        SERVICE_INSTANCE_ID,
        config
            .service_instance_id
            .clone()
            .unwrap_or_else(|| Uuid::now_v7().to_string()),
    )];
    let name = config
        .service_name
        .clone()
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
    // The SDK's environment detector runs after the one reading
    // `OTEL_SERVICE_NAME`, so a `service.name` in the attributes would win
    // there, against the specification.
    let service_name = read(OTEL_SERVICE_NAME).map(|name| KeyValue::new(SERVICE_NAME, name));
    Resource::builder_empty()
        .with_schema_url(Vec::<KeyValue>::new(), SCHEMA_URL)
        .with_attributes(ours)
        .with_attributes(over)
        .with_attributes(service_name)
        .build()
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

    /// `build` mutates `opentelemetry::global`, so every `build()` in this file is
    /// serialized.
    static GLOBAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The variables `set` holds, and nothing else.
    fn reading(
        set: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<String> + Copy {
        move |name: &str| {
            set.iter()
                .find(|(variable, _)| *variable == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

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
            );
            assert_eq!(attribute(&r, SERVICE_NAME).as_deref(), Some("auth"));
            assert_eq!(attribute(&r, SERVICE_VERSION).as_deref(), Some("7.0.0"));

            let pinned = OpenTelemetryConfig {
                service_name: Some("configured-svc".into()),
                service_version: Some("1.2.3".into()),
                ..OpenTelemetryConfig::default()
            };
            let r = build_resource(&pinned, Some(&binary()), &nest_rs_config::env_var);
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
            );
            let name = attribute(&r, SERVICE_NAME).expect("the SDK always names one");
            assert!(name.starts_with("unknown_service"), "{name}");
            assert!(attribute(&r, SERVICE_VERSION).is_none());
            Ok(())
        });
    }

    /// What the SDK read from the specification's variables wins over the pin
    /// and the binary: an operator's `OTEL_SERVICE_NAME` is the name.
    #[test]
    fn the_specification_s_variables_name_the_service_over_the_pin_and_the_binary() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("OTEL_SERVICE_NAME", "from-the-operator");
            jail.set_env(
                "OTEL_RESOURCE_ATTRIBUTES",
                "service.version=9.9.9,deployment.environment.name=staging",
            );
            let pinned = OpenTelemetryConfig {
                service_name: Some("pinned".into()),
                service_version: Some("1.0.0".into()),
                deployment_environment: Some("production".into()),
                ..OpenTelemetryConfig::default()
            };
            let r = build_resource(&pinned, Some(&binary()), &nest_rs_config::env_var);
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
            jail.set_env("OTEL_RESOURCE_ATTRIBUTES", "service.name=from-attributes");
            let r = build_resource(
                &OpenTelemetryConfig::default(),
                Some(&binary()),
                &nest_rs_config::env_var,
            );
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
            jail.set_env("OTEL_RESOURCE_ATTRIBUTES", "service.name=from-attributes");
            let r = build_resource(
                &OpenTelemetryConfig::default(),
                Some(&binary()),
                &nest_rs_config::env_var,
            );
            assert_eq!(
                attribute(&r, SERVICE_NAME).as_deref(),
                Some("from-service-name")
            );
            Ok(())
        });
    }

    #[test]
    fn the_instance_id_is_pinned_or_a_fresh_v7_uuid_per_build() {
        figment::Jail::expect_with(|_| {
            let unpinned = OpenTelemetryConfig::default();
            let a = attribute(
                &build_resource(&unpinned, None, &nest_rs_config::env_var),
                SERVICE_INSTANCE_ID,
            )
            .expect("instance id present");
            let b = attribute(
                &build_resource(&unpinned, None, &nest_rs_config::env_var),
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
                    &build_resource(&pinned, None, &nest_rs_config::env_var),
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
        assert_eq!(exporters.summary.endpoints, ["<none>", "<none>", "<none>"]);
        shut(exporters);
    }

    #[test]
    fn a_pinned_base_endpoint_exports_every_signal_at_its_path() {
        let _guard = GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for base in ["http://localhost:4318", "http://localhost:4318/"] {
            let exporters = build_reading(&pinned_endpoint(base), None, nothing).expect("builds");
            assert!(exporters.meter_provider.is_some() && exporters.logger_provider.is_some());
            assert_eq!(
                exporters.summary.endpoints,
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
            exporters.summary.endpoints,
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
        assert_eq!(
            exporters.summary.endpoints[0],
            "http://traces:4318/v1/traces"
        );
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
        let mut ignored = Vec::new();
        let config = pinned_endpoint("http://collector:4318");
        assert!(
            export(
                Signal::Traces,
                &config,
                &reading(&[(OTEL_EXPORTER_OTLP_PROTOCOL, "http/protobuf")]),
                &mut ignored
            )
            .expect("the one it speaks")
            .is_some()
        );
        for (variable, protocol) in [
            (OTEL_EXPORTER_OTLP_PROTOCOL, "grpc"),
            (OTEL_EXPORTER_OTLP_METRICS_PROTOCOL, "http/json"),
        ] {
            let read = move |name: &str| (name == variable).then(|| protocol.to_owned());
            let refused = export(Signal::Metrics, &config, &read, &mut ignored)
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
        assert!(sampler(&pinned, &nothing).expect("a ratio").is_some());
        assert!(
            sampler(&pinned, &reading(&[(OTEL_TRACES_SAMPLER, "always_off")]))
                .expect("the SDK's")
                .is_none(),
            "the SDK read OTEL_TRACES_SAMPLER, and it stands"
        );
        for ratio in [f64::NAN, -0.5, 2.5] {
            let refused = sampler(
                &OpenTelemetryConfig {
                    trace_sample_ratio: ratio,
                    ..OpenTelemetryConfig::default()
                },
                &nothing,
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
        let mut ignored = Vec::new();
        assert_eq!(
            metric_interval(&pinned, &nothing, &mut ignored).expect("in range"),
            Duration::from_secs(30)
        );
        assert_eq!(
            metric_interval(
                &pinned,
                &reading(&[(OTEL_METRIC_EXPORT_INTERVAL, "5000")]),
                &mut ignored
            )
            .expect("in range"),
            Duration::from_secs(5)
        );
        assert!(ignored.is_empty());

        assert_eq!(
            metric_interval(
                &pinned,
                &reading(&[(OTEL_METRIC_EXPORT_INTERVAL, "5s")]),
                &mut ignored
            )
            .expect("passed over"),
            Duration::from_secs(30)
        );
        assert_eq!(ignored, [OTEL_METRIC_EXPORT_INTERVAL]);

        let refused = metric_interval(
            &pinned,
            &reading(&[(OTEL_METRIC_EXPORT_INTERVAL, "0")]),
            &mut Vec::new(),
        )
        .expect_err("a tight loop")
        .to_string();
        assert!(refused.contains(OTEL_METRIC_EXPORT_INTERVAL), "{refused}");
        let refused = metric_interval(
            &OpenTelemetryConfig {
                metric_interval: Duration::ZERO,
                ..OpenTelemetryConfig::default()
            },
            &nothing,
            &mut Vec::new(),
        )
        .expect_err("a pinned zero is held where it is spent")
        .to_string();
        assert!(refused.contains(PINNED_INTERVAL), "{refused}");
    }

    /// The SDK's order: the signal's variable, then the general one, in
    /// milliseconds, then its 10 s; a value that is not a whole number of
    /// milliseconds is passed over and named once.
    #[test]
    fn an_export_timeout_is_resolved_as_the_sdk_resolves_it() {
        let resolved = |set: &'static [(&'static str, &'static str)]| {
            let mut ignored = Vec::new();
            let timeouts = [
                OTEL_EXPORTER_OTLP_TRACES_TIMEOUT,
                OTEL_EXPORTER_OTLP_METRICS_TIMEOUT,
                OTEL_EXPORTER_OTLP_LOGS_TIMEOUT,
            ]
            .map(|signal| export_timeout(signal, &reading(set), &mut ignored));
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

        assert!(
            flushed.is_err(),
            "the final export never answered, so the flush cannot have succeeded",
        );
        assert!(
            took >= bound && took < bound + std::time::Duration::from_secs(1),
            "shutdown gave the export its bound and no more, took {took:?}",
        );
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
