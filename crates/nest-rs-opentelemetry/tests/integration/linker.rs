//! Covers `src/linker.rs` against `tracing-opentelemetry` as it is: the library
//! behaviour its rename works around (`.claude/upstream/tracing-opentelemetry.md`).

use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use crate::otlp::{Collector, trace_exports, trusting};

/// Fails the day the library applies a late `otel.name` itself.
#[test]
fn upstream_tracing_opentelemetry_still_drops_a_name_recorded_on_a_started_span() {
    let collector = Collector::serving();
    let otel = trusting(
        &collector,
        &[
            ("OTEL_METRICS_EXPORTER", "none"),
            ("OTEL_LOGS_EXPORTER", "none"),
        ],
    );

    let span = tracing::info_span!("probe.static", otel.name = tracing::field::Empty);
    // Reading the context starts the SDK's span, as the linker does.
    drop(span.context());
    span.record("otel.name", "probe.renamed");
    drop(span);
    drop(otel);

    let traces = trace_exports(&collector);
    assert!(
        traces.iter().any(|export| export.holds("probe.static")),
        "the span was exported: {traces:?}",
    );
    assert!(
        !traces.iter().any(|export| export.holds("probe.renamed")),
        "tracing-opentelemetry now applies a late `otel.name`: retire `rename` in \
         src/linker.rs and close .claude/upstream/tracing-opentelemetry.md",
    );
}
