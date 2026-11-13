//! The duration of every unit of work whose edge's telemetry conventions name
//! a histogram for it — `http.server.request.duration`,
//! `messaging.process.duration`, `mcp.server.operation.duration` — recorded
//! from the unit's span, from its opening to the end its edge records
//! (`record_end`), or to its close for a unit whose end is never recorded: a
//! child span or work carried past the answer holds the span open beyond it.
//!
//! The layer runs under a filter of its own, so a console filtered to `warn`
//! and a sampler keeping one trace in a hundred still count every unit. Which
//! units it records, and by which fields, is what each unit declares
//! (`DurationHistogram`), read as `operation_span!` creates the span; a failed
//! unit's `error.type`, which `operation_span!` declares on every unit, keys it
//! besides.

use std::fmt;
use std::time::Instant;

use nest_rs_core::duration_histogram::{DurationHistogram, MetricAttribute};
use nest_rs_core::operation_log::Unit;
use opentelemetry::{KeyValue, StringValue, Value};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_semantic_conventions::attribute::ERROR_TYPE;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::Interest;
use tracing::{Metadata, Subscriber};
use tracing_subscriber::Registry;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Context, Filter, Layer};
use tracing_subscriber::registry::{LookupSpan, SpanRef};

use crate::durations::Durations;

/// Records each measured unit's duration in the histogram it declares.
pub(crate) struct UnitDurations(Durations);

impl UnitDurations {
    /// Record on `provider`, under the framework's scope.
    pub(crate) fn new(provider: &SdkMeterProvider) -> Self {
        Self(Durations::new(provider))
    }
}

impl<S> Layer<S> for UnitDurations
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(declared) = measured() else {
            return;
        };
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut timing = Timing {
            opened: Instant::now(),
            declared,
            keyed: vec![None; declared.attributes().len() + 1],
        };
        attrs.record(&mut timing);
        span.extensions_mut().insert(timing);
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        if let Some(timing) = span.extensions_mut().get_mut::<Timing>() {
            values.record(timing);
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id) {
            self.end(&span);
        }
    }
}

impl UnitDurations {
    /// Record the duration of the unit `span` opened, once: from its opening
    /// to now.
    fn end<'a, R: LookupSpan<'a>>(&self, span: &SpanRef<'a, R>) {
        let Some(timing) = span.extensions_mut().remove::<Timing>() else {
            return;
        };
        let seconds = timing.opened.elapsed().as_secs_f64();
        self.0
            .record(timing.declared, seconds, &timing.into_attributes());
    }
}

/// End the duration of the unit `span` opened, where the subscriber holding
/// it records durations; what `init` seeds as `record_end`'s hook.
pub(crate) fn end_unit(span: &tracing::Span) {
    span.with_subscriber(|(id, dispatch)| {
        let (Some(durations), Some(registry)) = (
            dispatch.downcast_ref::<UnitDurations>(),
            dispatch.downcast_ref::<Registry>(),
        ) else {
            return;
        };
        if let Some(span) = registry.span(id) {
            durations.end(&span);
        }
    });
}

/// The duration the unit whose span is being created declares, if any.
fn measured() -> Option<&'static DurationHistogram> {
    nest_rs_core::__private::pending_unit().and_then(Unit::duration)
}

/// Admits the spans `operation_span!` opens for a unit declaring a duration,
/// and nothing else, whatever the console's filter says.
pub(crate) struct MeasuredUnits;

impl<S> Filter<S> for MeasuredUnits {
    fn enabled(&self, metadata: &Metadata<'_>, _: &Context<'_, S>) -> bool {
        metadata.is_span() && measured().is_some()
    }

    fn callsite_enabled(&self, metadata: &'static Metadata<'static>) -> Interest {
        // Whether a unit declares a duration is read as its span is created.
        if crate::unit_spans::is_unit_span(metadata) {
            Interest::sometimes()
        } else {
            Interest::never()
        }
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::INFO)
    }
}

/// A measured unit's span: when it opened, what it records in, and the value
/// each declared attribute, then `error.type`, holds so far — one slot each, so
/// every duration reaches the histogram keyed in one order.
struct Timing {
    opened: Instant,
    declared: &'static DurationHistogram,
    keyed: Vec<Option<Value>>,
}

impl Timing {
    /// The slot `field` keys the duration in, and the attribute it was
    /// declared as — none for `error.type`; `None` for a field keying nothing.
    fn slot(&self, field: &Field) -> Option<(usize, Option<&'static MetricAttribute>)> {
        let declared = self.declared.attributes();
        match declared
            .iter()
            .position(|attribute| attribute.field() == field.name())
        {
            Some(at) => Some((at, declared.get(at))),
            None => (field.name() == ERROR_TYPE).then_some((declared.len(), None)),
        }
    }

    /// Key the duration by `field` as the text `value`, read through its known
    /// set where it has one.
    fn text(&mut self, field: &Field, value: &str) {
        let Some((at, attribute)) = self.slot(field) else {
            return;
        };
        let keyed = match attribute.and_then(|attribute| attribute.known_value(value)) {
            Some(known) => StringValue::from(known),
            None => StringValue::from(value.to_owned()),
        };
        self.keyed[at] = Some(Value::String(keyed));
    }

    /// Key the duration by `field` as a value no string carries: as it is
    /// where the field is bounded, through its known set as written otherwise.
    fn scalar(&mut self, field: &Field, value: Value) {
        let Some((at, attribute)) = self.slot(field) else {
            return;
        };
        match attribute {
            Some(attribute) if !attribute.is_bounded() => self.text(field, &value.as_str()),
            _ => self.keyed[at] = Some(value),
        }
    }

    /// The attributes the duration is keyed by, in their declared order.
    fn into_attributes(self) -> Vec<KeyValue> {
        let keys = self
            .declared
            .attributes()
            .iter()
            .map(MetricAttribute::field)
            .chain([ERROR_TYPE]);
        keys.zip(self.keyed)
            .filter_map(|(key, value)| Some(KeyValue::new(key, value?)))
            .collect()
    }
}

impl Visit for Timing {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.text(field, value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if self.slot(field).is_some() {
            self.text(field, &format!("{value:?}"));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.scalar(field, Value::I64(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        let value =
            i64::try_from(value).map_or_else(|_| Value::from(value.to_string()), Value::I64);
        self.scalar(field, value);
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.scalar(field, Value::F64(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.scalar(field, Value::Bool(value));
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::Correlation;
    use nest_rs_core::operation_log::OTHER;
    use nestrs_unit_probe::{CODE, STEP, TIMED_DURATION, VERB};
    use opentelemetry_sdk::metrics::data::{
        AggregatedMetrics, Metric, MetricData, ResourceMetrics,
    };
    use tracing_subscriber::layer::SubscriberExt as _;

    use super::*;
    use crate::durations::{SCOPE, SECONDS};
    use crate::testing::{metered, points};

    /// Every export of what `work` recorded through the layer, beside a layer
    /// enabling every span, as a console or a trace export does.
    fn recorded(work: impl FnOnce()) -> Vec<ResourceMetrics> {
        nest_rs_core::__private::set_unit_end(end_unit);
        let (provider, exporter) = metered();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::layer::Identity::new())
            .with(UnitDurations::new(&provider).with_filter(MeasuredUnits));
        tracing::subscriber::with_default(subscriber, work);
        provider.force_flush().expect("the provider flushes");
        exporter
            .get_finished_metrics()
            .expect("the exporter holds what was flushed")
    }

    /// The probe's duration in `exported`, with the scope it was recorded
    /// under, or `None` when nothing recorded in it.
    fn timed(exported: &[ResourceMetrics]) -> Option<(&str, &Metric)> {
        exported
            .iter()
            .flat_map(ResourceMetrics::scope_metrics)
            .flat_map(|scope| {
                scope
                    .metrics()
                    .map(move |metric| (scope.scope().name(), metric))
            })
            .find(|(_, metric)| metric.name() == TIMED_DURATION.name())
    }

    fn attributes(set: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut attributes: Vec<_> = set
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        attributes.sort();
        attributes
    }

    #[test]
    fn a_timed_unit_records_its_duration_in_seconds_keyed_by_what_its_span_says() {
        let exported = recorded(|| {
            let span = nestrs_unit_probe::open_timed(&Correlation::minted(None), "fetch", "read");
            span.record(CODE, 503_u64);
        });
        let (scope, metric) = timed(&exported).expect("the unit's duration was recorded");

        assert_eq!(scope, SCOPE);
        assert_eq!(metric.unit(), SECONDS);
        assert_eq!(metric.description(), TIMED_DURATION.description());
        assert_eq!(
            points(&exported, TIMED_DURATION.name()),
            vec![(
                attributes(&[(STEP, "fetch"), (VERB, "read"), (CODE, "503")]),
                1
            )],
            "the code recorded after the opening keys it too",
        );
        let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data() else {
            unreachable!("read above");
        };
        let bounds: Vec<f64> = histogram
            .data_points()
            .flat_map(|point| point.bounds().collect::<Vec<_>>())
            .collect();
        assert_eq!(bounds, TIMED_DURATION.boundaries().to_vec());
    }

    #[test]
    fn a_value_outside_its_known_set_keys_the_duration_as_other() {
        let exported = recorded(|| {
            drop(nestrs_unit_probe::open_timed(
                &Correlation::minted(None),
                "fetch",
                "anything-a-caller-sends",
            ));
        });
        assert_eq!(
            points(&exported, TIMED_DURATION.name()),
            vec![(attributes(&[(STEP, "fetch"), (VERB, OTHER)]), 1)],
        );
    }

    #[test]
    fn two_units_keyed_alike_count_in_one_point() {
        let exported = recorded(|| {
            for _ in 0..2 {
                drop(nestrs_unit_probe::open_timed(
                    &Correlation::minted(None),
                    "fetch",
                    "write",
                ));
            }
        });
        assert_eq!(
            points(&exported, TIMED_DURATION.name()),
            vec![(attributes(&[(STEP, "fetch"), (VERB, "write")]), 2)],
        );
    }

    /// What the probe's duration summed in `exported`, in seconds.
    fn summed(exported: &[ResourceMetrics]) -> f64 {
        let (_, metric) = timed(exported).expect("the unit's duration was recorded");
        let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data() else {
            panic!("a duration is an f64 histogram");
        };
        histogram.data_points().map(|point| point.sum()).sum()
    }

    /// A child span, or work carried past the answer, holds the unit's span
    /// open; the duration ends where the unit's outcome is recorded.
    #[test]
    fn a_units_duration_ends_at_its_outcome_whatever_holds_its_span_open() {
        // The duration had the unit ended at its span's close: the bound.
        const HELD: std::time::Duration = std::time::Duration::from_millis(100);
        let exported = recorded(|| {
            let span = nestrs_unit_probe::open_timed(&Correlation::minted(None), "fetch", "read");
            let child = tracing::info_span!(parent: &span, "probe.child");
            nest_rs_core::operation_log::record_outcome(&span, nest_rs_core::operation_log::OK);
            drop(span);
            std::thread::sleep(HELD);
            drop(child);
        });

        let seconds = summed(&exported);
        assert!(seconds < HELD.as_secs_f64(), "{seconds} s");
    }

    #[test]
    fn a_unit_declaring_no_duration_and_a_span_no_unit_opens_record_nothing() {
        let exported = recorded(|| {
            drop(nestrs_unit_probe::open(&Correlation::minted(None)));
            drop(tracing::info_span!(
                "library",
                parent_span_id = "a library's field"
            ));
        });

        assert!(timed(&exported).is_none(), "{exported:?}");
    }
}
