//! Covers `src/unit_spans.rs` as `init` installs it: every unit's span reaches
//! the trace export whatever the console writes, and a span the console's
//! filter closes stays out of it.

use nest_rs_core::Correlation;

use crate::otlp::{Collector, Received, trusting};

/// Every export of traces `collector` read.
fn trace_exports(collector: &Collector) -> Vec<Received> {
    collector
        .received()
        .into_iter()
        .filter(|export| export.path == "/v1/traces")
        .collect()
}

/// Open the probe's unit under a console filtered to `warn`, with an `info`
/// span inside it, and return what the collector read of the traces.
fn under_warn(metrics: &str) -> Vec<Received> {
    let collector = Collector::serving();
    let otel = trusting(
        &collector,
        &[
            ("RUST_LOG", "warn"),
            ("OTEL_METRICS_EXPORTER", metrics),
            ("OTEL_LOGS_EXPORTER", "none"),
        ],
    );

    let unit = nestrs_unit_probe::open(&Correlation::minted(None));
    unit.in_scope(|| {
        drop(tracing::info_span!("probe.inside").entered());
        drop(
            tracing::info_span!(
                target: "probe_library",
                "probe.lookalike",
                parent_span_id = "a library's own field",
            )
            .entered(),
        );
    });
    drop(unit);
    drop(otel);

    trace_exports(&collector)
}

#[test]
fn a_units_span_is_exported_with_the_console_filtered_to_warn() {
    let exports = under_warn("none");

    assert!(
        exports
            .iter()
            .any(|export| export.holds(nestrs_unit_probe::WORK.name())),
        "the unit's span reached the collector: {exports:?}",
    );
}

#[test]
fn a_units_span_is_exported_with_the_console_filtered_to_warn_where_durations_count() {
    let exports = under_warn("otlp");

    assert!(
        exports
            .iter()
            .any(|export| export.holds(nestrs_unit_probe::WORK.name())),
        "the unit's span reached the collector: {exports:?}",
    );
}

#[test]
fn a_span_the_console_filter_closes_stays_out_of_the_trace_export() {
    let exports = under_warn("none");

    assert!(
        !exports.iter().any(|export| export.holds("probe.inside")),
        "the `info` span stayed out: {exports:?}",
    );
}

/// A library's span declaring a field named as a unit's is no unit: the
/// console's filter, and its clamps, decide it.
#[test]
fn a_span_shaped_like_a_units_outside_operation_span_stays_out_of_the_trace_export() {
    let exports = under_warn("none");

    assert!(
        !exports.iter().any(|export| export.holds("probe.lookalike")),
        "the lookalike stayed out: {exports:?}",
    );
}
