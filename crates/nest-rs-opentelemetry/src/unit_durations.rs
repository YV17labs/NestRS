//! The duration of every unit of work whose edge's telemetry conventions name
//! a histogram for it — `http.server.request.duration`,
//! `messaging.process.duration`, `mcp.server.operation.duration` — recorded
//! from the unit's span, from its opening to its close.
//!
//! The layer runs under a filter of its own, so a console filtered to `warn`
//! and a sampler keeping one trace in a hundred still count every unit. Which
//! units it records, and by which fields, is what each unit declares
//! (`UnitDuration`), read as `operation_span!` creates the span.

use std::fmt;
use std::time::Instant;

use nest_rs_core::operation_log::{MetricAttribute, Unit, UnitDuration};
use opentelemetry::{KeyValue, StringValue, Value};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::Interest;
use tracing::{Metadata, Subscriber};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Context, Filter, Layer};
use tracing_subscriber::registry::LookupSpan;

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
        let Some(duration) = measured() else {
            return;
        };
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut timing = Timing {
            opened: Instant::now(),
            duration,
            attributes: Vec::with_capacity(duration.attributes().len()),
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
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let Some(timing) = span.extensions_mut().remove::<Timing>() else {
            return;
        };
        self.0
            .histogram(timing.duration.name(), timing.duration.description())
            .record(timing.opened.elapsed().as_secs_f64(), &timing.attributes);
    }
}

/// The duration the unit whose span is being created declares, if any.
fn measured() -> Option<&'static UnitDuration> {
    nest_rs_core::__private::pending_unit().and_then(Unit::duration)
}

/// Admits the spans `operation_span!` opens for a unit declaring a duration,
/// and nothing else, whatever the console's filter says — nothing at all where
/// no metric exports.
pub(crate) struct MeasuredUnits {
    measuring: bool,
}

impl MeasuredUnits {
    /// The filter of a binary whose metrics export when `measuring`.
    pub(crate) const fn when(measuring: bool) -> Self {
        Self { measuring }
    }
}

impl<S> Filter<S> for MeasuredUnits {
    fn enabled(&self, metadata: &Metadata<'_>, _: &Context<'_, S>) -> bool {
        self.measuring && metadata.is_span() && measured().is_some()
    }

    fn callsite_enabled(&self, metadata: &'static Metadata<'static>) -> Interest {
        // Only `operation_span!` declares the parent's id: every other callsite
        // is settled once, here.
        let operation = metadata.is_span()
            && metadata
                .fields()
                .field(nest_rs_core::__private::field::PARENT_SPAN_ID)
                .is_some();
        if self.measuring && operation {
            Interest::sometimes()
        } else {
            Interest::never()
        }
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(if self.measuring {
            LevelFilter::INFO
        } else {
            LevelFilter::OFF
        })
    }
}

/// A measured unit's span: when it opened, what it records in, and the
/// attributes its duration is keyed by so far.
struct Timing {
    opened: Instant,
    duration: &'static UnitDuration,
    attributes: Vec<KeyValue>,
}

impl Timing {
    /// The attribute `field` keys the duration as, if it keys it at all.
    fn attribute(&self, field: &Field) -> Option<&'static MetricAttribute> {
        self.duration
            .attributes()
            .iter()
            .find(|attribute| attribute.field() == field.name())
    }

    /// Key the duration by `field` as `value`, over what it said before.
    fn key(&mut self, field: &Field, value: Value) {
        let keyed = KeyValue::new(field.name(), value);
        match self
            .attributes
            .iter_mut()
            .find(|attribute| attribute.key.as_str() == field.name())
        {
            Some(attribute) => *attribute = keyed,
            None => self.attributes.push(keyed),
        }
    }

    /// Key the duration by a value no string carries: as it is where the span
    /// bounds it, through its known set as written otherwise.
    fn scalar(&mut self, field: &Field, value: Value) {
        let Some(attribute) = self.attribute(field) else {
            return;
        };
        if attribute.is_bounded() {
            self.key(field, value);
        } else {
            self.record_str(field, &value.as_str());
        }
    }
}

impl Visit for Timing {
    fn record_str(&mut self, field: &Field, value: &str) {
        let Some(attribute) = self.attribute(field) else {
            return;
        };
        let value = match attribute.known_value(value) {
            Some(known) => StringValue::from(known),
            None => StringValue::from(value.to_owned()),
        };
        self.key(field, Value::String(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if self.attribute(field).is_some() {
            self.record_str(field, &format!("{value:?}"));
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
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader};
    use tracing_subscriber::layer::SubscriberExt as _;

    use super::*;
    use crate::durations::{REQUEST_BOUNDARIES, SCOPE, SECONDS};

    /// One data point of the probe's duration: its attributes, sorted, and
    /// how many units it counts.
    type Point = (Vec<(String, String)>, u64);

    /// Every export of what `work` recorded through the layer.
    fn recorded(work: impl FnOnce()) -> Vec<ResourceMetrics> {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        let subscriber = tracing_subscriber::registry()
            .with(UnitDurations::new(&provider).with_filter(MeasuredUnits::when(true)));
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

    /// The data points of a duration, as [`Point`]s.
    fn points(metric: &Metric) -> Vec<Point> {
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
            .collect()
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
            points(metric),
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
        assert_eq!(bounds, REQUEST_BOUNDARIES.to_vec());
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
        let (_, metric) = timed(&exported).expect("the unit's duration was recorded");

        assert_eq!(
            points(metric),
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
        let (_, metric) = timed(&exported).expect("the units' duration was recorded");

        assert_eq!(
            points(metric),
            vec![(attributes(&[(STEP, "fetch"), (VERB, "write")]), 2)],
        );
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
