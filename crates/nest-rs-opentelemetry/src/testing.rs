//! Test doubles: an environment holding a fixed set of variables, and a meter
//! provider whose exports a test reads.

/// The variables `set` holds, and nothing else.
pub(crate) fn reading(set: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let set: Vec<(String, String)> = set
        .iter()
        .map(|(variable, value)| ((*variable).to_owned(), (*value).to_owned()))
        .collect();
    move |name: &str| {
        set.iter()
            .find(|(variable, _)| variable == name)
            .map(|(_, value)| value.clone())
    }
}

/// A meter provider whose exports an in-memory exporter keeps.
#[cfg(feature = "otlp")]
pub(crate) fn metered() -> (
    opentelemetry_sdk::metrics::SdkMeterProvider,
    opentelemetry_sdk::metrics::InMemoryMetricExporter,
) {
    let exporter = opentelemetry_sdk::metrics::InMemoryMetricExporter::default();
    let provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
        .with_reader(opentelemetry_sdk::metrics::PeriodicReader::builder(exporter.clone()).build())
        .build();
    (provider, exporter)
}

/// One data point of a duration: its attributes, sorted, and how many
/// durations it counts.
#[cfg(feature = "otlp")]
pub(crate) type Point = (Vec<(String, String)>, u64);

/// Each data point of the duration `name` in `exported`.
#[cfg(feature = "otlp")]
pub(crate) fn points(
    exported: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
    name: &str,
) -> Vec<Point> {
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};

    named(exported, name)
        .flat_map(|metric| {
            let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data() else {
                panic!("a duration is an f64 histogram: {metric:?}");
            };
            histogram
                .data_points()
                .map(|point| (sorted(point.attributes()), point.count()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// One data point of a sum — a counter's or an up-down counter's, observed or
/// not: its attributes, sorted, and its value as text.
#[cfg(feature = "otlp")]
pub(crate) type SumPoint = (Vec<(String, String)>, String);

/// Each data point of the sum `name` in `exported`.
#[cfg(feature = "otlp")]
pub(crate) fn sums(
    exported: &[opentelemetry_sdk::metrics::data::ResourceMetrics],
    name: &str,
) -> Vec<SumPoint> {
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, Sum};

    fn of<T: Copy + ToString>(sum: &Sum<T>) -> Vec<SumPoint> {
        sum.data_points()
            .map(|point| (sorted(point.attributes()), point.value().to_string()))
            .collect()
    }

    named(exported, name)
        .flat_map(|metric| match metric.data() {
            AggregatedMetrics::I64(MetricData::Sum(sum)) => of(sum),
            AggregatedMetrics::U64(MetricData::Sum(sum)) => of(sum),
            AggregatedMetrics::F64(MetricData::Sum(sum)) => of(sum),
            other => panic!("{name} is a sum: {other:?}"),
        })
        .collect()
}

/// Every metric named `name` in `exported`.
#[cfg(feature = "otlp")]
fn named<'a>(
    exported: &'a [opentelemetry_sdk::metrics::data::ResourceMetrics],
    name: &'a str,
) -> impl Iterator<Item = &'a opentelemetry_sdk::metrics::data::Metric> {
    exported
        .iter()
        .flat_map(opentelemetry_sdk::metrics::data::ResourceMetrics::scope_metrics)
        .flat_map(|scope| scope.metrics())
        .filter(move |metric| metric.name() == name)
}

/// `attributes` as text, sorted.
#[cfg(feature = "otlp")]
fn sorted<'a>(
    attributes: impl Iterator<Item = &'a opentelemetry::KeyValue>,
) -> Vec<(String, String)> {
    let mut attributes: Vec<_> = attributes
        .map(|kv| (kv.key.to_string(), kv.value.to_string()))
        .collect();
    attributes.sort();
    attributes
}
