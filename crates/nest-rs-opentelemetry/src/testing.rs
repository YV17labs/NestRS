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
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};

    exported
        .iter()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(|scope| scope.metrics())
        .filter(|metric| metric.name() == name)
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
