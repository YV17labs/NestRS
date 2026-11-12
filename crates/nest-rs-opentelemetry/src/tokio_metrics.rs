//! The async runtime's own metrics — its workers, its live tasks, the queue
//! every worker steals from and how long they have been busy — read when the
//! metric reader exports, off tokio's stable runtime metrics.
//!
//! No OpenTelemetry convention names a tokio runtime, so the names follow the
//! conventions' runtime metrics (`dotnet.thread_pool.*`, `jvm.thread.count`):
//! `tokio.` and the runtime's own word for each thing.

use opentelemetry::metrics::Meter;
use tokio::runtime::{Handle, RuntimeMetrics};

use crate::durations::SECONDS;

/// Registers the runtime's instruments.
pub(crate) struct TokioMetrics;

/// One of the runtime's counts: its name, unit and description, and its read.
type Count = (
    &'static str,
    &'static str,
    &'static str,
    fn(&RuntimeMetrics) -> usize,
);

/// The runtime's counts, each read off its metrics.
const COUNTS: [Count; 3] = [
    (
        "tokio.worker.count",
        "{worker}",
        "The runtime's worker threads.",
        RuntimeMetrics::num_workers,
    ),
    (
        "tokio.task.count",
        "{task}",
        "The tasks alive on the runtime.",
        RuntimeMetrics::num_alive_tasks,
    ),
    (
        "tokio.global_queue.length",
        "{task}",
        "The tasks waiting in the queue every worker takes from.",
        RuntimeMetrics::global_queue_depth,
    ),
];

impl TokioMetrics {
    /// Register on `meter` the instruments of the runtime `init` runs on; none
    /// outside one.
    pub(crate) fn observe(meter: &Meter) {
        let Ok(handle) = Handle::try_current() else {
            return;
        };
        let runtime = handle.metrics();
        for (name, unit, description, read) in COUNTS {
            let runtime = runtime.clone();
            meter
                .i64_observable_up_down_counter(name)
                .with_unit(unit)
                .with_description(description)
                .with_callback(move |observer| observer.observe(count(read(&runtime)), &[]))
                .build();
        }
        meter
            .f64_observable_counter("tokio.worker.busy_time")
            .with_unit(SECONDS)
            .with_description("The time the runtime's workers spent running tasks.")
            .with_callback(move |observer| {
                let busy: f64 = (0..runtime.num_workers())
                    .map(|worker| runtime.worker_total_busy_duration(worker).as_secs_f64())
                    .sum();
                observer.observe(busy, &[]);
            })
            .build();
    }
}

fn count(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

    use super::*;

    #[test]
    fn the_runtime_init_runs_on_is_read_when_the_metrics_export() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a runtime");
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        runtime.block_on(async { TokioMetrics::observe(&provider.meter("runtime")) });
        provider.force_flush().expect("the provider flushes");

        let exported = exporter.get_finished_metrics().expect("exported");
        let value = |name: &str| -> f64 {
            let metric = exported
                .iter()
                .flat_map(|resource| resource.scope_metrics())
                .flat_map(|scope| scope.metrics())
                .find(|metric| metric.name() == name)
                .unwrap_or_else(|| panic!("{name} was exported"));
            match metric.data() {
                AggregatedMetrics::I64(MetricData::Sum(sum)) => {
                    #[expect(clippy::cast_precision_loss, reason = "a test's small counts")]
                    let value = sum.data_points().map(|point| point.value()).sum::<i64>() as f64;
                    value
                }
                AggregatedMetrics::F64(MetricData::Sum(sum)) => {
                    sum.data_points().map(|point| point.value()).sum()
                }
                other => panic!("{name}: {other:?}"),
            }
        };
        assert_eq!(value("tokio.worker.count"), 2.0);
        assert!(value("tokio.task.count") >= 0.0);
        assert!(value("tokio.global_queue.length") >= 0.0);
        assert!(value("tokio.worker.busy_time") >= 0.0);
    }

    #[test]
    fn outside_a_runtime_nothing_is_registered() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        TokioMetrics::observe(&provider.meter("runtime"));
        provider.force_flush().expect("the provider flushes");
        assert!(
            exporter
                .get_finished_metrics()
                .expect("exported")
                .iter()
                .all(|resource| resource.scope_metrics().next().is_none()),
        );
    }
}
