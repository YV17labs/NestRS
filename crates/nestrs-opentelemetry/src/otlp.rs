//! OTel SDK wiring. Always installs a local tracer + W3C propagator so
//! `tracing` spans get trace ids and `traceparent` propagates without an
//! exporter. When an OTLP endpoint is set, attaches batch exporters for
//! traces/metrics/logs over HTTP/protobuf.

use opentelemetry::KeyValue;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{LogExporter, MetricExporter, Protocol, SpanExporter, WithExportConfig};
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
}

pub(crate) fn build(config: &OpenTelemetryConfig) -> Result<Exporters, OpenTelemetryError> {
    global::set_text_map_propagator(TraceContextPropagator::new());

    let resource = build_resource(config);
    let sampler = Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
        config.trace_sample_ratio,
    )));

    let mut tracer_builder = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_sampler(sampler);

    let endpoint = config
        .otlp_endpoint
        .as_deref()
        .map(|s| s.trim_end_matches('/'));

    if let Some(base) = endpoint {
        let span_exporter = SpanExporter::builder()
            .with_http()
            .with_endpoint(format!("{}/v1/traces", base))
            .with_protocol(Protocol::HttpBinary)
            .build()
            .map_err(|e| OpenTelemetryError::Otlp(e.to_string()))?;
        tracer_builder = tracer_builder.with_batch_exporter(span_exporter);
    }

    let tracer_provider = tracer_builder.build();
    let tracer = tracer_provider.tracer(config.service_name.clone());
    global::set_tracer_provider(tracer_provider.clone());

    let (meter_provider, logger_provider) = if let Some(base) = endpoint {
        let metric_exporter = MetricExporter::builder()
            .with_http()
            .with_endpoint(format!("{}/v1/metrics", base))
            .with_protocol(Protocol::HttpBinary)
            .build()
            .map_err(|e| OpenTelemetryError::Otlp(e.to_string()))?;
        let reader = PeriodicReader::builder(metric_exporter).build();
        let meter_provider = SdkMeterProvider::builder()
            .with_resource(resource.clone())
            .with_reader(reader)
            .build();
        global::set_meter_provider(meter_provider.clone());

        let log_exporter = LogExporter::builder()
            .with_http()
            .with_endpoint(format!("{}/v1/logs", base))
            .with_protocol(Protocol::HttpBinary)
            .build()
            .map_err(|e| OpenTelemetryError::Otlp(e.to_string()))?;
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
    })
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
    Resource::builder()
        .with_schema_url(attrs, SCHEMA_URL)
        .build()
}
