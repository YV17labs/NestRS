//! Covers `src/process_metrics.rs` and `src/tokio_metrics.rs` as `init`
//! installs them: once the metrics export, the process's and the runtime's own
//! instruments reach the collector, read at the export.

use opentelemetry_semantic_conventions::metric::{PROCESS_CPU_TIME, PROCESS_UPTIME};

use crate::otlp::{Collector, metric_exports, trusting};

#[test]
fn the_process_and_the_runtime_init_runs_on_are_exported_with_the_metrics() {
    let collector = Collector::serving();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");
    let otel = runtime.block_on(async {
        trusting(
            &collector,
            &[
                ("OTEL_TRACES_EXPORTER", "none"),
                ("OTEL_LOGS_EXPORTER", "none"),
            ],
        )
    });
    drop(otel);

    let exports = metric_exports(&collector);
    for metric in [
        PROCESS_CPU_TIME,
        PROCESS_UPTIME,
        "tokio.worker.count",
        "tokio.worker.busy_time",
    ] {
        assert!(
            exports.iter().any(|export| export.holds(metric)),
            "{metric} reached the collector: {exports:?}",
        );
    }
}
