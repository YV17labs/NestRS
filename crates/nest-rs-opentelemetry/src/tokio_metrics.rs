//! The async runtime's own metrics — its workers, its live tasks, the queue
//! every worker steals from and how long they have been busy — read when the
//! metric reader exports, off tokio's stable runtime metrics.
//!
//! No OpenTelemetry convention names a tokio runtime, so the names follow the
//! conventions' runtime metrics (`dotnet.thread_pool.*`, `jvm.thread.count`):
//! `tokio.` and the runtime's own word for each thing.

use opentelemetry::metrics::Meter;
use tokio::runtime::Handle;

/// Registers the runtime's instruments.
pub(crate) struct TokioMetrics;

impl TokioMetrics {
    /// Register on `meter` the instruments of the runtime `init` runs on; none
    /// outside one.
    pub(crate) fn observe(meter: &Meter) {
        let Ok(handle) = Handle::try_current() else {
            return;
        };
        let runtime = handle.metrics();
        let workers = runtime.clone();
        meter
            .i64_observable_up_down_counter("tokio.worker.count")
            .with_unit("{worker}")
            .with_description("The runtime's worker threads.")
            .with_callback(move |observer| observer.observe(count(workers.num_workers()), &[]))
            .build();
        let tasks = runtime.clone();
        meter
            .i64_observable_up_down_counter("tokio.task.count")
            .with_unit("{task}")
            .with_description("The tasks alive on the runtime.")
            .with_callback(move |observer| observer.observe(count(tasks.num_alive_tasks()), &[]))
            .build();
        let queue = runtime.clone();
        meter
            .i64_observable_up_down_counter("tokio.global_queue.length")
            .with_unit("{task}")
            .with_description("The tasks waiting in the queue every worker takes from.")
            .with_callback(move |observer| observer.observe(count(queue.global_queue_depth()), &[]))
            .build();
        meter
            .f64_observable_counter("tokio.worker.busy_time")
            .with_unit("s")
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
