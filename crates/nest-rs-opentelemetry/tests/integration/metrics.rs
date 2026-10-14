//! Covers `src/metrics.rs` and `src/instruments/` — what an app records through
//! `Metrics`, read back from the SDK's in-memory exporter behind the global
//! meter provider the module's `Metrics` records on.

use std::sync::Arc;

use nest_rs_core::App;
use nest_rs_opentelemetry::{
    Attribute, Describe, Metrics, OpenTelemetry, OpenTelemetryConfig, OpenTelemetryModule,
};
use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, Metric, MetricData, ResourceMetrics};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

/// The app's `Metrics`, recording on a provider whose exports the test reads.
struct Recorded {
    metrics: Arc<Metrics>,
    provider: SdkMeterProvider,
    exporter: InMemoryMetricExporter,
    _otel: OpenTelemetry,
}

impl Recorded {
    fn boot() -> Self {
        let otel = OpenTelemetry::init_with(OpenTelemetryConfig::new("metrics"))
            .expect("the console-only init");
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());
        let app = App::new::<OpenTelemetryModule>().expect("the module boots after init");
        let metrics = app
            .container()
            .get::<Metrics>()
            .expect("the module provides Metrics");
        Self {
            metrics,
            provider,
            exporter,
            _otel: otel,
        }
    }

    /// Every metric exported so far, the last export of each name.
    fn exported(&self) -> Vec<ResourceMetrics> {
        self.provider.force_flush().expect("the provider flushes");
        self.exporter
            .get_finished_metrics()
            .expect("the exporter holds what was flushed")
    }
}

fn key_values(attributes: impl Iterator<Item = KeyValue>) -> Vec<(String, String)> {
    attributes
        .map(|kv| (kv.key.to_string(), kv.value.to_string()))
        .collect()
}

#[test]
fn a_counter_exports_one_sum_with_its_attributes() {
    let recorded = Recorded::boot();
    recorded
        .metrics
        .counter("orders.placed")
        .add(2, &[Attribute::new("source", "api")]);

    let exported = recorded.exported();
    let metric = find(&exported, "orders.placed");
    let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
        panic!("a counter exports a u64 sum: {metric:?}")
    };
    assert!(sum.is_monotonic());
    let points: Vec<_> = sum.data_points().collect();
    assert_eq!(points.len(), 1, "{points:?}");
    assert_eq!(points[0].value(), 2);
    assert_eq!(
        key_values(points[0].attributes().cloned()),
        vec![("source".to_owned(), "api".to_owned())]
    );
}

#[test]
fn an_up_down_counter_takes_a_negative() {
    let recorded = Recorded::boot();
    let queued = recorded.metrics.up_down_counter("jobs.queued");
    queued.add(3, &[]);
    queued.add(-5, &[]);

    let exported = recorded.exported();
    let metric = find(&exported, "jobs.queued");
    let AggregatedMetrics::I64(MetricData::Sum(sum)) = metric.data() else {
        panic!("an up-down counter exports an i64 sum: {metric:?}")
    };
    assert!(!sum.is_monotonic());
    assert_eq!(
        sum.data_points().map(|point| point.value()).sum::<i64>(),
        -2
    );
}

#[test]
fn a_histogram_exports_its_records_into_the_bounds_it_was_declared_with() {
    let recorded = Recorded::boot();
    let latency = recorded.metrics.histogram_with(
        "checkout.latency",
        Describe::default()
            .with_unit("ms")
            .with_description("checkout latency"),
        &[1.0, 5.0],
    );
    latency.record(0.5, &[Attribute::new("route", "/checkout")]);
    latency.record(3.0, &[Attribute::new("route", "/checkout")]);
    latency.record(9.0, &[Attribute::new("route", "/checkout")]);

    let exported = recorded.exported();
    let metric = find(&exported, "checkout.latency");
    assert_eq!(metric.unit(), "ms");
    assert_eq!(metric.description(), "checkout latency");
    let AggregatedMetrics::F64(MetricData::Histogram(histogram)) = metric.data() else {
        panic!("a histogram exports an f64 histogram: {metric:?}")
    };
    let point = histogram.data_points().next().expect("one point");
    assert_eq!(point.bounds().collect::<Vec<_>>(), vec![1.0, 5.0]);
    assert_eq!(point.bucket_counts().collect::<Vec<_>>(), vec![1, 1, 1]);
    assert_eq!(point.count(), 3);
}

#[test]
fn a_gauge_exports_its_last_value() {
    let recorded = Recorded::boot();
    let depth = recorded.metrics.gauge("pool.idle");
    depth.record(4.0, &[Attribute::new("pool", "primary")]);
    depth.record(2.0, &[Attribute::new("pool", "primary")]);

    let exported = recorded.exported();
    let metric = find(&exported, "pool.idle");
    let AggregatedMetrics::F64(MetricData::Gauge(gauge)) = metric.data() else {
        panic!("a gauge exports an f64 gauge: {metric:?}")
    };
    let values: Vec<f64> = gauge.data_points().map(|point| point.value()).collect();
    assert_eq!(values, vec![2.0]);
}

/// A handle kept in a field and one asked for again record on one instrument.
#[test]
fn two_handles_on_one_name_record_on_one_instrument() {
    let recorded = Recorded::boot();
    let kept = recorded.metrics.counter("users.created");
    kept.add(1, &[]);
    recorded.metrics.counter("users.created").add(1, &[]);
    kept.clone().add(1, &[]);

    let exported = recorded.exported();
    let metric = find(&exported, "users.created");
    let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
        panic!("a counter exports a u64 sum: {metric:?}")
    };
    assert_eq!(sum.data_points().map(|point| point.value()).sum::<u64>(), 3);
}

/// Every kind of attribute value reaches the export as the value it was given.
#[test]
fn every_attribute_value_is_exported_as_given() {
    let recorded = Recorded::boot();
    recorded.metrics.counter("attributes").add(
        1,
        &[
            Attribute::new("flag", true),
            Attribute::new("count", 7_i64),
            Attribute::new("ratio", 0.5),
            Attribute::new("static", "text"),
            Attribute::new("owned", String::from("built")),
        ],
    );

    let exported = recorded.exported();
    let metric = find(&exported, "attributes");
    let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
        panic!("a counter exports a u64 sum: {metric:?}")
    };
    let mut attributes = key_values(
        sum.data_points()
            .next()
            .expect("one point")
            .attributes()
            .cloned(),
    );
    attributes.sort();
    assert_eq!(
        attributes,
        [
            ("count", "7"),
            ("flag", "true"),
            ("owned", "built"),
            ("ratio", "0.5"),
            ("static", "text"),
        ]
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
    );
}

/// The metric named `name` in the last export that holds it.
fn find<'a>(exported: &'a [ResourceMetrics], name: &str) -> &'a Metric {
    exported
        .iter()
        .rev()
        .flat_map(ResourceMetrics::scope_metrics)
        .flat_map(|scope| scope.metrics())
        .find(|metric| metric.name() == name)
        .unwrap_or_else(|| panic!("{name} was exported"))
}
