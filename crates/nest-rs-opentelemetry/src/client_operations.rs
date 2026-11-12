//! What records a client operation a driver timed — a statement, a command —
//! as the client span its conventions name, under the unit that sent it, and in
//! the duration histogram they name for it.
//!
//! An operation sent outside every traced unit — a worker's poll, a relay's
//! claim — records its duration and no span: a root span per poll would bury
//! the traces an operator reads.

use std::sync::OnceLock;

use nest_rs_core::client_operation::{ClientOperation, ClientValue};
use opentelemetry::trace::{
    Span as _, SpanKind, Status, TraceContextExt as _, Tracer as _, TracerProvider as _,
};
use opentelemetry::{InstrumentationScope, KeyValue, StringValue, Value};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::{SdkTracerProvider, Tracer};
use opentelemetry_semantic_conventions::attribute::ERROR_TYPE;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use crate::durations::{Durations, SCOPE};

/// What records each client operation: spans where the traces export, the
/// duration where the metrics do.
struct ClientOperations {
    tracer: Option<Tracer>,
    durations: Option<Durations>,
}

static RECORDING: OnceLock<ClientOperations> = OnceLock::new();

/// Record every client operation a driver times — as a span on `traces`, in
/// its histogram on `metrics` — once; with neither, nothing is seeded and no
/// driver times anything.
pub(crate) fn install(traces: Option<&SdkTracerProvider>, metrics: Option<&SdkMeterProvider>) {
    if traces.is_none() && metrics.is_none() {
        return;
    }
    let recording = ClientOperations {
        tracer: traces.map(|provider| {
            provider.tracer_with_scope(
                InstrumentationScope::builder(SCOPE)
                    .with_version(env!("CARGO_PKG_VERSION"))
                    .build(),
            )
        }),
        durations: metrics.map(Durations::new),
    };
    if RECORDING.set(recording).is_ok() {
        nest_rs_core::__private::set_client_recorder(record);
    }
}

fn record(operation: &ClientOperation<'_>) {
    if let Some(recording) = RECORDING.get() {
        recording.record(operation);
    }
}

impl ClientOperations {
    fn record(&self, operation: &ClientOperation<'_>) {
        if let Some(durations) = &self.durations {
            let keys = operation.duration().keys();
            let mut keyed: Vec<KeyValue> = operation
                .attributes()
                .iter()
                .filter(|(key, _)| keys.contains(key))
                .map(|(key, value)| KeyValue::new(*key, value_of(*value)))
                .collect();
            if let Some(error_type) = operation.error_type() {
                keyed.push(KeyValue::new(ERROR_TYPE, error_type.to_owned()));
            }
            durations
                .histogram(
                    operation.duration().name(),
                    operation.duration().description(),
                )
                .record(operation.elapsed().as_secs_f64(), &keyed);
        }
        if let Some(tracer) = &self.tracer {
            span(tracer, operation);
        }
    }
}

/// The operation's client span, under the unit's span that sent it — none
/// where no traced unit did.
fn span(tracer: &Tracer, operation: &ClientOperation<'_>) {
    let parent = tracing::Span::current().context();
    if !parent.span().span_context().is_valid() {
        return;
    }
    let mut attributes: Vec<KeyValue> = operation
        .attributes()
        .iter()
        .map(|(key, value)| KeyValue::new(*key, value_of(*value)))
        .collect();
    if let Some(error_type) = operation.error_type() {
        attributes.push(KeyValue::new(ERROR_TYPE, error_type.to_owned()));
    }
    let builder = tracer
        .span_builder(operation.name().to_owned())
        .with_kind(SpanKind::Client)
        .with_start_time(operation.started())
        .with_attributes(attributes);
    // The id generator adopts the ids published while the span starts: the
    // span the peer was told is its parent is this one.
    let mut span = match operation.announced() {
        Some(announced) => nest_rs_core::__private::with_pending_ids(announced, || {
            builder.start_with_context(tracer, &parent)
        }),
        None => builder.start_with_context(tracer, &parent),
    };
    if operation.error_type().is_some() {
        span.set_status(Status::error(""));
    }
    span.end_with_timestamp(operation.started() + operation.elapsed());
}

fn value_of(value: ClientValue<'_>) -> Value {
    match value {
        ClientValue::Integer(integer) => Value::I64(integer),
        ClientValue::Text(text) => Value::String(StringValue::from(text.to_owned())),
        _ => Value::String(StringValue::from(nest_rs_core::operation_log::OTHER)),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use nest_rs_core::Correlation;
    use nest_rs_core::client_operation::ClientDuration;
    use opentelemetry::trace::SpanKind;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SpanData};
    use tracing_subscriber::layer::SubscriberExt as _;

    use super::*;

    const STATEMENT: ClientDuration = ClientDuration::new(
        "db.client.operation.duration",
        "Duration of database client operations.",
        &["db.system.name", "db.operation.name"],
    );

    /// What one recorder exported: its spans, and its metrics.
    struct Exported {
        spans: Vec<SpanData>,
        metrics: Vec<ResourceMetrics>,
    }

    /// Record what `work` sends, as a unit's span opened by the framework does
    /// and a recorder seeded on both signals would.
    fn exported(work: impl FnOnce(&ClientOperations)) -> Exported {
        let spans = InMemorySpanExporter::default();
        let tracer_provider = SdkTracerProvider::builder()
            .with_id_generator(crate::id_generator::AdoptFrameworkIds::default())
            .with_simple_exporter(spans.clone())
            .build();
        let metrics = InMemoryMetricExporter::default();
        let meter_provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(metrics.clone()).build())
            .build();
        let recording = ClientOperations {
            tracer: Some(tracer_provider.tracer("operations")),
            durations: Some(Durations::new(&meter_provider)),
        };
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer("units")));
        tracing::subscriber::with_default(subscriber, || work(&recording));
        tracer_provider.force_flush().expect("the spans flush");
        meter_provider.force_flush().expect("the metrics flush");
        Exported {
            spans: spans.get_finished_spans().expect("the spans exported"),
            metrics: metrics
                .get_finished_metrics()
                .expect("the metrics exported"),
        }
    }

    fn select(recording: &ClientOperations, failed: bool) {
        let attributes = [
            ("db.system.name", ClientValue::Text("postgresql")),
            ("db.operation.name", ClientValue::Text("SELECT")),
            ("db.query.summary", ClientValue::Text("SELECT posts")),
            ("server.port", ClientValue::Integer(5432)),
        ];
        let operation = ClientOperation::new(
            &STATEMENT,
            "SELECT posts",
            SystemTime::now() - Duration::from_millis(4),
            Duration::from_millis(4),
            &attributes,
        );
        recording.record(&if failed {
            operation.failed("_OTHER")
        } else {
            operation
        });
    }

    /// The duration's points, each as its sorted attributes and its count.
    fn points(metrics: &[ResourceMetrics]) -> Vec<(Vec<(String, String)>, u64)> {
        metrics
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(|scope| scope.metrics())
            .filter(|metric| metric.name() == STATEMENT.name())
            .flat_map(|metric| {
                let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data() else {
                    panic!("a duration is an f64 histogram: {metric:?}");
                };
                histogram
                    .data_points()
                    .map(|point| {
                        let mut attributes: Vec<_> = point
                            .attributes()
                            .map(|kv| (kv.key.to_string(), kv.value.to_string()))
                            .collect();
                        attributes.sort();
                        (attributes, point.count())
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn attribute(span: &SpanData, key: &str) -> Option<String> {
        span.attributes
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| kv.value.to_string())
    }

    #[test]
    fn an_operation_inside_a_unit_is_its_client_span_and_its_duration() {
        let exported = exported(|recording| {
            let unit = nestrs_unit_probe::open(&Correlation::minted(None));
            unit.in_scope(|| select(recording, false));
        });

        let unit = exported
            .spans
            .iter()
            .find(|span| span.span_kind != SpanKind::Client)
            .expect("the unit's span");
        let operation = exported
            .spans
            .iter()
            .find(|span| span.span_kind == SpanKind::Client)
            .expect("the operation's span");
        assert_eq!(operation.name, "SELECT posts");
        assert_eq!(operation.parent_span_id, unit.span_context.span_id());
        assert_eq!(
            operation.span_context.trace_id(),
            unit.span_context.trace_id()
        );
        assert_eq!(
            operation
                .end_time
                .duration_since(operation.start_time)
                .expect("ends after it starts"),
            Duration::from_millis(4),
        );
        assert_eq!(
            attribute(operation, "db.query.summary").as_deref(),
            Some("SELECT posts"),
            "the span carries every attribute",
        );
        assert_eq!(attribute(operation, "server.port").as_deref(), Some("5432"));
        assert_eq!(attribute(operation, ERROR_TYPE), None);

        assert_eq!(
            points(&exported.metrics),
            vec![(
                vec![
                    ("db.operation.name".to_owned(), "SELECT".to_owned()),
                    ("db.system.name".to_owned(), "postgresql".to_owned()),
                ],
                1
            )],
            "the duration is keyed by its keys alone",
        );
    }

    #[test]
    fn an_operation_outside_every_unit_records_its_duration_and_no_span() {
        let exported = exported(|recording| select(recording, false));

        assert!(exported.spans.is_empty(), "{:?}", exported.spans);
        assert_eq!(points(&exported.metrics).len(), 1);
    }

    #[test]
    fn a_failed_operation_says_its_class_on_its_span_and_its_duration() {
        let exported = exported(|recording| {
            let unit = nestrs_unit_probe::open(&Correlation::minted(None));
            unit.in_scope(|| select(recording, true));
        });

        let operation = exported
            .spans
            .iter()
            .find(|span| span.span_kind == SpanKind::Client)
            .expect("the operation's span");
        assert_eq!(attribute(operation, ERROR_TYPE).as_deref(), Some("_OTHER"));
        assert!(matches!(operation.status, Status::Error { .. }));
        assert!(points(&exported.metrics).iter().all(|(attributes, _)| {
            attributes.contains(&(ERROR_TYPE.to_owned(), "_OTHER".to_owned()))
        }),);
    }

    #[test]
    fn an_operation_announced_to_its_peer_is_the_span_the_peer_was_told_of() {
        let announced = Correlation::minted(None).child();
        let exported = exported(|recording| {
            let unit = nestrs_unit_probe::open(&Correlation::minted(None));
            unit.in_scope(|| {
                let attributes = [("http.request.method", ClientValue::Text("GET"))];
                recording.record(
                    &ClientOperation::new(
                        &STATEMENT,
                        "GET",
                        SystemTime::now(),
                        Duration::from_millis(1),
                        &attributes,
                    )
                    .announced_as(&announced),
                );
            });
        });

        let operation = exported
            .spans
            .iter()
            .find(|span| span.span_kind == SpanKind::Client)
            .expect("the operation's span");
        assert_eq!(
            operation.span_context.span_id().to_string(),
            announced.span_id().to_hex(),
        );
    }
}
