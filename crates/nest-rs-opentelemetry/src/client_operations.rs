//! What records a client operation a client timed — a statement, a command, a
//! request — as the client span its conventions name, under the unit that sent
//! it, and in the duration histogram they name for it.
//!
//! An operation sent outside every traced unit — a worker's poll, a relay's
//! claim — records its duration and no span: a root span per poll would bury
//! the traces an operator reads. The unit is the kernel's correlation the
//! operation carries, never the `tracing` span entered: the ids a
//! `traceparent` announced are the correlation's, and a task can run a unit
//! with no span entered.

use std::sync::OnceLock;

use nest_rs_core::client_operation::{ClientOperation, ClientValue};
use nest_rs_core::duration_histogram::MetricAttribute;
use nest_rs_core::operation_log::OTHER;
use opentelemetry::trace::{
    Span as _, SpanKind, Status, TraceFlags, Tracer as _, TracerProvider as _,
};
use opentelemetry::{KeyValue, StringValue, Value};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::{SdkTracerProvider, Tracer};
use opentelemetry_semantic_conventions::attribute::ERROR_TYPE;

use crate::durations::{Durations, framework_scope};

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
        tracer: traces.map(|provider| provider.tracer_with_scope(framework_scope())),
        durations: metrics.map(Durations::new),
    };
    let durations = recording.durations.is_some();
    if RECORDING.set(recording).is_ok() {
        nest_rs_core::__private::set_client_recorder(record, durations);
    }
}

fn record(operation: &ClientOperation<'_>) {
    if let Some(recording) = RECORDING.get() {
        recording.record(operation);
    }
}

impl ClientOperations {
    fn record(&self, operation: &ClientOperation<'_>) {
        let keyed = keyed_attributes(operation);
        if let Some(durations) = &self.durations {
            durations.record(
                operation.duration(),
                operation.elapsed().as_secs_f64(),
                &keyed,
            );
        }
        if let Some(tracer) = &self.tracer {
            span(tracer, operation, keyed);
        }
    }
}

/// What the operation's duration is keyed by: the attributes it declares, read
/// as it declares them, then `error.type`.
fn keyed_attributes(operation: &ClientOperation<'_>) -> Vec<KeyValue> {
    let declared = operation.duration().attributes();
    let mut keyed: Vec<KeyValue> = operation
        .attributes()
        .iter()
        .filter_map(|(key, value)| {
            let attribute = declared
                .iter()
                .find(|attribute| attribute.field() == *key)?;
            Some(KeyValue::new(*key, keyed(attribute, *value)))
        })
        .collect();
    if let Some(error_type) = operation.error_type() {
        keyed.push(KeyValue::new(ERROR_TYPE, error_type.to_owned()));
    }
    keyed
}

/// The operation's client span, at the place in its unit's trace the operation
/// carries — none outside every unit, or where the unit's trace is not sampled
/// — carrying `keyed` and every attribute its duration is not keyed by.
fn span(tracer: &Tracer, operation: &ClientOperation<'_>, mut attributes: Vec<KeyValue>) {
    let Some(own) = operation.correlation() else {
        return;
    };
    if !own.flags().is_sampled() {
        return;
    }
    let Some(parent) = crate::linker::parent_context(own, TraceFlags::SAMPLED, false) else {
        return;
    };
    let declared = operation.duration().attributes();
    attributes.extend(
        operation
            .attributes()
            .iter()
            .filter(|(key, _)| declared.iter().all(|attribute| attribute.field() != *key))
            .map(|(key, value)| KeyValue::new(*key, value_of(*value))),
    );
    let builder = tracer
        .span_builder(operation.name().to_owned())
        .with_kind(SpanKind::Client)
        .with_start_time(operation.started())
        .with_attributes(attributes);
    // The id generator adopts the ids published while the span starts: the
    // span a peer was told is its parent is this one.
    let mut span = nest_rs_core::__private::with_pending_ids(own, || {
        builder.start_with_context(tracer, &parent)
    });
    if operation.error_type().is_some() {
        span.set_status(Status::error(""));
    }
    span.end_with_timestamp(operation.started() + operation.elapsed());
}

/// `value` as the duration is keyed by `attribute`: as it is where the
/// attribute is bounded, through its known set as written otherwise.
fn keyed(attribute: &MetricAttribute, value: ClientValue<'_>) -> Value {
    if attribute.is_bounded() {
        return value_of(value);
    }
    let known = match value {
        ClientValue::Text(text) => attribute.known_value(text),
        other => attribute.known_value(&other.to_string()),
    };
    Value::from(known.unwrap_or(OTHER))
}

fn value_of(value: ClientValue<'_>) -> Value {
    match value {
        ClientValue::Integer(integer) => Value::I64(integer),
        ClientValue::Text(text) => Value::String(StringValue::from(text.to_owned())),
        other => Value::String(StringValue::from(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use nest_rs_core::client_operation::{
        DB_CLIENT_OPERATION_DURATION, DB_OPERATION_NAME, DB_QUERY_SUMMARY, DB_SYSTEM_NAME,
        SERVER_PORT,
    };
    use nest_rs_core::duration_histogram::DurationHistogram;
    use opentelemetry::trace::SpanKind;
    use opentelemetry_sdk::metrics::data::ResourceMetrics;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SpanData};
    use tracing_subscriber::layer::SubscriberExt as _;

    use nest_rs_core::Correlation;

    use super::*;
    use crate::testing::{Point, metered};

    /// What one recorder exported: its spans, and its metrics.
    struct Exported {
        spans: Vec<SpanData>,
        metrics: Vec<ResourceMetrics>,
    }

    /// Record what `work` sends, as a recorder seeded on both signals would,
    /// under a subscriber exporting the framework's spans, each unit's span
    /// started under the kernel's ids as `init` links them.
    fn exported(work: impl FnOnce(&ClientOperations)) -> Exported {
        crate::linker::install();
        let spans = InMemorySpanExporter::default();
        let tracer_provider = SdkTracerProvider::builder()
            .with_id_generator(crate::id_generator::AdoptFrameworkIds::default())
            .with_simple_exporter(spans.clone())
            .build();
        let (meter_provider, metrics) = metered();
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

    /// Run `send` inside a unit of work, its span entered, as an edge runs a
    /// handler.
    fn in_unit(send: impl FnOnce()) {
        run_in(&Correlation::minted(None), true, send);
    }

    /// Run `send` inside the unit of work `correlation` names, its span
    /// `entered` or opened and left aside, as a task spawned under the unit's
    /// scope runs.
    fn run_in(correlation: &Correlation, entered: bool, send: impl FnOnce()) {
        let unit = nestrs_unit_probe::open(correlation);
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime")
            .block_on(nest_rs_core::with_request_scope(
                None,
                correlation.clone(),
                async {
                    if entered {
                        unit.in_scope(send);
                    } else {
                        send();
                    }
                },
            ));
    }

    /// The one client span `exported` holds.
    fn client_span(exported: &Exported) -> &SpanData {
        exported
            .spans
            .iter()
            .find(|span| span.span_kind == SpanKind::Client)
            .expect("the operation's span")
    }

    /// A statement sent from the unit this task runs, placed in it as
    /// `ClientOperation::record` places one its client gave no place.
    fn select(recording: &ClientOperations, failure: Option<&str>) {
        let attributes = [
            (DB_SYSTEM_NAME, ClientValue::Text("postgresql")),
            (DB_OPERATION_NAME, ClientValue::Text("SELECT")),
            (DB_QUERY_SUMMARY, ClientValue::Text("SELECT posts")),
            (SERVER_PORT, ClientValue::Integer(5432)),
            ("an.attribute.no.duration.keys", ClientValue::Text("kept")),
        ];
        let own = nest_rs_core::__private::current_correlation().map(|unit| unit.child());
        recording.record(
            &ClientOperation::new(
                &DB_CLIENT_OPERATION_DURATION,
                "SELECT posts",
                SystemTime::now() - Duration::from_millis(4),
                Duration::from_millis(4),
                &attributes,
            )
            .failed(failure)
            .correlated(own.as_ref()),
        );
    }

    fn points(exported: &Exported) -> Vec<Point> {
        crate::testing::points(&exported.metrics, DB_CLIENT_OPERATION_DURATION.name())
    }

    fn attribute(span: &SpanData, key: &str) -> Option<String> {
        span.attributes
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| kv.value.to_string())
    }

    fn pairs(set: &[(&str, &str)]) -> Vec<(String, String)> {
        set.iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn an_operation_inside_a_unit_is_its_client_span_and_its_duration() {
        let exported = exported(|recording| in_unit(|| select(recording, None)));

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
            attribute(operation, "an.attribute.no.duration.keys").as_deref(),
            Some("kept"),
            "the span carries every attribute",
        );
        assert_eq!(attribute(operation, ERROR_TYPE), None);

        assert_eq!(
            points(&exported),
            vec![(
                pairs(&[
                    (DB_OPERATION_NAME, "SELECT"),
                    (DB_QUERY_SUMMARY, "SELECT posts"),
                    (DB_SYSTEM_NAME, "postgresql"),
                    (SERVER_PORT, "5432"),
                ]),
                1
            )],
            "the duration is keyed by its declared attributes alone",
        );
    }

    #[test]
    fn an_operation_outside_every_unit_records_its_duration_and_no_span() {
        let exported = exported(|recording| select(recording, None));

        assert!(exported.spans.is_empty(), "{:?}", exported.spans);
        assert_eq!(points(&exported).len(), 1);
    }

    #[test]
    fn a_failed_operation_says_its_class_on_its_span_and_its_duration() {
        let exported = exported(|recording| in_unit(|| select(recording, Some("_OTHER"))));

        let operation = exported
            .spans
            .iter()
            .find(|span| span.span_kind == SpanKind::Client)
            .expect("the operation's span");
        assert_eq!(attribute(operation, ERROR_TYPE).as_deref(), Some("_OTHER"));
        assert!(matches!(operation.status, Status::Error { .. }));
        assert!(points(&exported).iter().all(|(attributes, _)| {
            attributes.contains(&(ERROR_TYPE.to_owned(), "_OTHER".to_owned()))
        }));
    }

    #[test]
    fn an_operation_correlated_by_its_client_is_the_span_its_peer_was_told_of() {
        let correlation = Correlation::minted(None);
        let child = correlation.child();
        let exported = exported(|recording| {
            run_in(&correlation, true, || {
                recording.record(
                    &ClientOperation::new(
                        &DB_CLIENT_OPERATION_DURATION,
                        "GET",
                        SystemTime::now(),
                        Duration::from_millis(1),
                        &[],
                    )
                    .correlated(Some(&child)),
                );
            });
        });

        let operation = client_span(&exported);
        assert_eq!(
            operation.span_context.span_id().to_string(),
            child.span_id().to_hex(),
        );
        assert_eq!(
            operation.parent_span_id.to_string(),
            correlation.span_id().to_hex()
        );
    }

    #[test]
    fn an_operation_in_a_unit_whose_span_is_not_entered_is_its_client_span_under_the_unit() {
        let correlation = Correlation::minted(None);
        let exported =
            exported(|recording| run_in(&correlation, false, || select(recording, None)));

        let operation = client_span(&exported);
        assert_eq!(
            operation.span_context.trace_id().to_string(),
            correlation.trace_id().to_hex()
        );
        assert_eq!(
            operation.parent_span_id.to_string(),
            correlation.span_id().to_hex()
        );
    }

    #[test]
    fn an_operation_under_a_root_span_of_its_own_stays_in_its_units_trace() {
        let correlation = Correlation::minted(None);
        let exported = exported(|recording| {
            run_in(&correlation, true, || {
                tracing::info_span!(parent: None, "detached").in_scope(|| select(recording, None));
            });
        });

        let operation = client_span(&exported);
        assert_eq!(
            operation.span_context.trace_id().to_string(),
            correlation.trace_id().to_hex(),
            "the unit's trace, not the detached span's",
        );
        assert_eq!(
            operation.parent_span_id.to_string(),
            correlation.span_id().to_hex()
        );
    }

    #[test]
    fn a_known_attribute_holding_an_integer_keys_its_duration_through_its_set() {
        const CODE: &str = "probe.code";
        const CODED: DurationHistogram = DurationHistogram::new(
            "probe.coded.duration",
            "Duration of a coded probe.",
            &[1.0],
            &[MetricAttribute::known(CODE, &["200"])],
        );
        let coded = |recording: &ClientOperations, code: i64| {
            recording.record(&ClientOperation::new(
                &CODED,
                "CODED",
                SystemTime::now(),
                Duration::from_millis(1),
                &[(CODE, ClientValue::Integer(code))],
            ));
        };

        let exported = exported(|recording| {
            coded(recording, 200);
            coded(recording, 999);
        });

        let mut points = crate::testing::points(&exported.metrics, CODED.name());
        points.sort();
        assert_eq!(
            points,
            vec![(pairs(&[(CODE, "200")]), 1), (pairs(&[(CODE, OTHER)]), 1)],
            "a value outside the set keys no series of its own",
        );
    }
}
