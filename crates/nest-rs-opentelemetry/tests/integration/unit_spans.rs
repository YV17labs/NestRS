//! Covers `src/unit_spans.rs` as `init` installs it: every unit's span reaches
//! the trace export whatever the console writes, and a span the console's
//! filter closes stays out of it.

use nest_rs_core::Correlation;

use crate::otlp::{Collector, Received, trace_exports, trusting};

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

/// A unit's span starts as it opens, the sampler's verdict read off it; the name
/// its edge learns later — a request's route — still names the exported span.
#[test]
fn a_name_recorded_after_the_unit_opened_names_its_exported_span() {
    let collector = Collector::serving();
    let otel = trusting(
        &collector,
        &[
            ("OTEL_METRICS_EXPORTER", "none"),
            ("OTEL_LOGS_EXPORTER", "none"),
        ],
    );

    let unit = nestrs_unit_probe::open(&Correlation::minted(None));
    nest_rs_core::operation_log::record_name(&unit, format_args!("GET /probe/{{id}}"));
    drop(unit);
    drop(otel);

    let exports = trace_exports(&collector);
    assert!(
        exports.iter().any(|export| export.holds("GET /probe/{id}")),
        "the span was renamed: {exports:?}",
    );
}

/// The words a failure line inside the probe's unit carries in `error`, which
/// nothing else the export holds says.
const FAILED_BECAUSE: &str = "the ledger at the eu-west warehouse refused";

/// The words an `info` line inside the probe's unit carries, below the filter.
const NOTED_BECAUSE: &str = "the ledger answered from its replica";

/// Open the probe's unit under a console filtered to `warn`, traces exported
/// alone, with an `error` line and an `info` line filed inside it — no cause
/// recorded on the span — and return what the collector read of the traces.
fn lines_under_warn() -> Vec<Received> {
    let collector = Collector::serving();
    let otel = trusting(
        &collector,
        &[
            ("RUST_LOG", "warn"),
            ("OTEL_METRICS_EXPORTER", "none"),
            ("OTEL_LOGS_EXPORTER", "none"),
        ],
    );

    let unit = nestrs_unit_probe::open(&Correlation::minted(None));
    unit.in_scope(|| {
        tracing::error!(target: "probe", error = FAILED_BECAUSE, "probe work failed");
        tracing::info!(target: "probe", error = NOTED_BECAUSE, "probe work noted");
    });
    drop(unit);
    drop(otel);

    trace_exports(&collector)
}

/// A line the console's filter admits, filed inside a unit's span, leaves with
/// it as one of its events, fields included: its words reach the trace
/// backend with logs exported nowhere.
#[test]
fn a_line_filed_in_a_units_span_leaves_with_it_as_an_event_under_traces_alone() {
    let exports = lines_under_warn();

    assert!(
        exports
            .iter()
            .any(|export| export.holds("probe work failed") && export.holds(FAILED_BECAUSE)),
        "the line and its `error` reached the collector: {exports:?}",
    );
}

/// A line the console's filter closes leaves no event on the span it was filed
/// in.
#[test]
fn a_line_the_console_filter_closes_leaves_no_event_on_its_units_span() {
    let exports = lines_under_warn();

    assert!(
        exports
            .iter()
            .any(|export| export.holds("probe work failed")),
        "the span and its admitted line reached the collector: {exports:?}",
    );
    assert!(
        !exports
            .iter()
            .any(|export| export.holds("probe work noted") || export.holds(NOTED_BECAUSE)),
        "the `info` line stayed out: {exports:?}",
    );
}

/// An app's `info` span, and a library's, under a console filter that quiets
/// everything to `warn` but names the app's target, as the traces page says to
/// write it, returned with what the collector read of the traces.
fn spans_under_warn_naming_the_app() -> Vec<Received> {
    let collector = Collector::serving();
    let otel = trusting(
        &collector,
        &[
            ("RUST_LOG", "warn,features=info"),
            ("OTEL_METRICS_EXPORTER", "none"),
            ("OTEL_LOGS_EXPORTER", "none"),
        ],
    );

    drop(tracing::info_span!(target: "features::users", "users.lookup").entered());
    drop(tracing::info_span!(target: "probe_library", "library.lookup").entered());
    drop(otel);

    trace_exports(&collector)
}

/// `RUST_LOG=warn,features=info` keeps the console quiet and the app's own
/// spans exported, and opens nothing else.
#[test]
fn an_apps_span_its_target_names_is_exported_under_a_console_quieted_to_warn() {
    let exports = spans_under_warn_naming_the_app();

    assert!(
        exports.iter().any(|export| export.holds("users.lookup")),
        "the app's span reached the collector: {exports:?}",
    );
    assert!(
        !exports.iter().any(|export| export.holds("library.lookup")),
        "a target the filter does not name stayed out: {exports:?}",
    );
}
