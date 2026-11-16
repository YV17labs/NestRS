//! Covers `src/linker.rs` against `tracing-opentelemetry` as it is: the library
//! behaviour its rename works around (`.claude/upstream/tracing-opentelemetry.md`),
//! where no trace is exported, what a unit still holds — the sampler's verdict
//! on its correlation, whence the flag of every `traceparent` it sends — and the
//! context an app reads inside it, the unit's only where traces export.

use nest_rs_core::{Correlation, TraceParent, TraceState};
use nest_rs_opentelemetry::OpenTelemetryConfig;
use opentelemetry::trace::TraceContextExt as _;
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

/// Whether a unit opened under `correlation`, with `init` reading `vars` and no
/// endpoint set, is sampled: the flag its correlation carries, and every
/// `traceparent` it sends with it.
fn sampled_with_nothing_exported(vars: &[(&str, &str)], correlation: &Correlation) -> bool {
    let otel =
        crate::init_under(vars, OpenTelemetryConfig::default()).expect("init with no endpoint");
    let unit = nestrs_unit_probe::open(correlation);
    let sampled = correlation.flags().is_sampled();
    drop(unit);
    drop(otel);
    sampled
}

/// A trace continued from a parent whose trace id is `trace_id`.
fn continued(trace_id: &str) -> Correlation {
    let parent = TraceParent::parse(&format!("00-{trace_id}-00f067aa0ba902b7-01"))
        .expect("a well-formed traceparent");
    Correlation::continued(parent, TraceState::default(), None)
}

#[test]
fn with_nothing_exported_a_unit_is_sampled_under_always_on() {
    let vars = [("OTEL_TRACES_SAMPLER", "always_on")];
    assert!(sampled_with_nothing_exported(
        &vars,
        &Correlation::minted(None)
    ));
}

#[test]
fn with_nothing_exported_a_unit_is_unsampled_under_always_off() {
    let vars = [("OTEL_TRACES_SAMPLER", "always_off")];
    assert!(!sampled_with_nothing_exported(
        &vars,
        &Correlation::minted(None)
    ));
}

/// `traceidratio` reads the trace id's last eight bytes against the ratio: all
/// zero falls under one half, all one over it.
#[test]
fn with_nothing_exported_a_unit_is_sampled_as_a_ratio_reads_its_trace_id() {
    let otel = crate::init_under(
        &[
            ("OTEL_TRACES_SAMPLER", "traceidratio"),
            ("OTEL_TRACES_SAMPLER_ARG", "0.5"),
        ],
        OpenTelemetryConfig::default(),
    )
    .expect("init with no endpoint");
    let under = continued("4bf92f3577b34da60000000000000000");
    let over = continued("4bf92f3577b34da6ffffffffffffffff");
    let units = [
        nestrs_unit_probe::open(&under),
        nestrs_unit_probe::open(&over),
    ];
    let verdicts = [under.flags().is_sampled(), over.flags().is_sampled()];
    drop(units);
    drop(otel);
    assert_eq!(verdicts, [true, false]);
}

/// The console's filter decides what is written, never what is sampled: a
/// unit under `RUST_LOG=warn` carries the sampler's verdict all the same.
#[test]
fn with_nothing_exported_and_the_console_quieted_a_unit_is_sampled_under_always_on() {
    let vars = [("OTEL_TRACES_SAMPLER", "always_on"), ("RUST_LOG", "warn")];
    assert!(sampled_with_nothing_exported(
        &vars,
        &Correlation::minted(None)
    ));
}

/// A trace the framework starts unsampled — a health probe's — stays so under
/// a sampler that would keep every other.
#[test]
fn with_nothing_exported_a_trace_started_unsampled_stays_unsampled() {
    let vars = [("OTEL_TRACES_SAMPLER", "always_on")];
    let probe = nest_rs_core::__private::minted_unsampled();
    assert!(!sampled_with_nothing_exported(&vars, &probe));
}

/// A trace started unsampled stays so where the console's filter closed its
/// unit's span, the sampler asked without one.
#[test]
fn with_nothing_exported_and_the_console_quieted_a_trace_started_unsampled_stays_unsampled() {
    let vars = [("OTEL_TRACES_SAMPLER", "always_on"), ("RUST_LOG", "warn")];
    let probe = nest_rs_core::__private::minted_unsampled();
    assert!(!sampled_with_nothing_exported(&vars, &probe));
}

/// What an app reading `opentelemetry::Context::current()` inside a unit opened
/// under `correlation` reads, with `otel` installed.
fn context_read_in_a_unit(
    otel: nest_rs_opentelemetry::OpenTelemetry,
    correlation: &Correlation,
) -> opentelemetry::trace::SpanContext {
    let unit = nestrs_unit_probe::open(correlation);
    let read = unit.in_scope(|| {
        opentelemetry::Context::current()
            .span()
            .span_context()
            .clone()
    });
    drop(unit);
    drop(otel);
    read
}

/// Where traces export, an app reading `opentelemetry::Context::current()`
/// inside a unit reads that unit's trace and span.
#[test]
fn with_traces_exported_an_app_reads_its_units_context() {
    let collector = Collector::serving();
    let otel = trusting(
        &collector,
        &[
            ("OTEL_METRICS_EXPORTER", "none"),
            ("OTEL_LOGS_EXPORTER", "none"),
        ],
    );
    let correlation = Correlation::minted(None);
    let read = context_read_in_a_unit(otel, &correlation);

    assert_eq!(
        read.trace_id().to_bytes(),
        correlation.trace_id().to_bytes()
    );
    assert_eq!(read.span_id().to_bytes(), correlation.span_id().to_bytes());
}

/// Where no trace exports, no span is built for OpenTelemetry, so an app
/// reading `opentelemetry::Context::current()` inside a unit reads none: its
/// trace is `nest_rs::core::current_traceparent()`
/// (`.claude/decisions/telemetry-reads-the-specification.md`, 2026-11-16).
#[test]
fn with_nothing_exported_an_app_reads_no_span_in_its_unit() {
    let otel =
        crate::init_under(&[], OpenTelemetryConfig::default()).expect("init with no endpoint");
    let correlation = Correlation::minted(None);
    let read = context_read_in_a_unit(otel, &correlation);

    assert!(!read.is_valid(), "{read:?}");
}
