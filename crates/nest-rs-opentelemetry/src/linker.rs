//! What this crate adds to a span the framework already opened — at **every**
//! edge, not one: every edge opens its unit of work through `operation_span!`,
//! which calls [`link_span`](nest_rs_core::__private::link_span).
//!
//! A seeded function pointer, because both steps need the `tracing::Span`
//! handle, which exists only inside the macro, in a crate that must not depend
//! on this one: `tracing_opentelemetry::OtelData` is private and an
//! `IdGenerator` never sees a span.

use std::sync::{Once, OnceLock};

use opentelemetry::Context;
use opentelemetry::trace::{SpanContext, SpanId, SpanKind, TraceContextExt, TraceFlags, TraceId};
use opentelemetry_sdk::trace::{Sampler, SamplingDecision, ShouldSample};
use tracing_opentelemetry::{OpenTelemetrySpanExt, SetParentError};

use crate::sampler::AdoptUnsampledStarts;

use nest_rs_core::Correlation;
use nest_rs_core::operation_log::Unit;

/// Install the framework's span linker before any module registers: where
/// `traces` export, [`enrich`], and [`rename`] as what renames a unit's span
/// its edge names late; otherwise [`record_verdict`], no SDK span existing to
/// link, read or rename.
pub(crate) fn install(traces: bool) {
    if traces {
        nest_rs_core::__private::set_span_linker(enrich);
        nest_rs_core::__private::set_span_rename(rename);
    } else {
        nest_rs_core::__private::set_span_linker(record_verdict);
    }
}

/// The sampler the tracer provider was built with, asked directly where a
/// unit's span holds no SDK span to read its verdict off.
static SAMPLER: OnceLock<AdoptUnsampledStarts<Sampler>> = OnceLock::new();

/// Hand [`enrich`] the sampler the tracer provider was built with.
pub(crate) fn sample_with(sampler: AdoptUnsampledStarts<Sampler>) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "seeded once at boot; a second install keeps the first"
    )]
    let _ = SAMPLER.set(sampler);
}

/// Install [`end`] as what runs as a unit ends: for every unit where `traces`
/// export, whose span counts the client spans it did not make; for a unit
/// declaring a duration where only `metrics` do; for none otherwise.
pub(crate) fn end_units(traces: bool, metrics: bool) {
    if traces || metrics {
        nest_rs_core::__private::set_unit_end(end, traces);
    }
}

/// End the duration `unit` declares, and say on its span how many client
/// spans it did not make.
fn end(unit: &Unit, span: &tracing::Span) {
    if unit.duration().is_some() {
        crate::unit_durations::end_unit(span);
    }
    crate::client_operations::report_dropped(span);
}

/// Rename the SDK's span: [`enrich`] started it, and `tracing-opentelemetry`
/// applies no `otel.name` recorded on a started span
/// (`.claude/upstream/tracing-opentelemetry.md`).
fn rename(span: &tracing::Span, name: String) {
    span.context().span().update_name(name);
}

/// Link the remote parent, then record what the sampler decided.
///
/// The order is load-bearing: a `ParentBased` sampler reads the parent, so a
/// verdict read before the link is the verdict for a trace this span is not in.
/// Reading the verdict starts the SDK's span, which `tracing-opentelemetry`
/// defers until then; the framework runs this while its ids are pending, so the
/// span starts under them.
fn enrich(span: &tracing::Span, correlation: &Correlation) {
    link_remote_parent(span, correlation);
    let sampled = sdk_verdict(span).unwrap_or_else(|| decided(correlation));
    nest_rs_core::__private::set_sampled(correlation, sampled);
    if sampled {
        crate::client_operations::hold_count(span, correlation);
    }
}

/// Record what the sampler decides for the unit `correlation` names, where no
/// trace exports: whence the flag of every `traceparent` it sends.
fn record_verdict(_: &tracing::Span, correlation: &Correlation) {
    nest_rs_core::__private::set_sampled(correlation, decided(correlation));
}

/// Tell the SDK this span continues one that ran elsewhere.
///
/// Only where the framework continued a trace: a **restarted** trace has no
/// parent, and inventing one would rejoin the trace a trust gate refused.
/// Whether the parent is remote is the correlation's to say: a WS message's ran
/// in this process. The link is explicit even for a local parent, since a WS
/// message and an MCP operation run on a task their parent's span never entered.
fn link_remote_parent(span: &tracing::Span, correlation: &Correlation) {
    let Some(parent) = parent_context(
        correlation,
        TraceFlags::new(correlation.flags().bits()),
        correlation.parent_is_remote(),
    ) else {
        return;
    };
    if let Err(error) = span.set_parent(parent)
        && is_structural(&error)
    {
        static REPORTED: Once = Once::new();
        REPORTED.call_once(|| {
            tracing::warn!(
                target: crate::TARGET,
                error = %nest_rs_core::error_message(&error),
                "remote parent not linked; continued traces export without their parent",
            );
        });
    }
}

/// The context of the span `correlation` is a child of — its trace, its
/// parent, its vendor state — flagged `flags`, remote as `remote` says; `None`
/// for a trace that starts at `correlation`.
pub(crate) fn parent_context(
    correlation: &Correlation,
    flags: TraceFlags,
    remote: bool,
) -> Option<Context> {
    let parent = SpanContext::new(
        TraceId::from_bytes(correlation.trace_id().to_bytes()),
        SpanId::from_bytes(correlation.parent_id()?.to_bytes()),
        flags,
        remote,
        // The SDK's own `FromStr`, rather than a private splitter that would drift.
        correlation
            .tracestate()
            .as_str()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or_default(),
    );
    Some(Context::new().with_remote_span_context(parent))
}

/// Whether a refused link says something about the wiring rather than the span:
/// a span the layer's own filter disabled has no parent to miss.
fn is_structural(error: &SetParentError) -> bool {
    !matches!(error, SetParentError::SpanDisabled)
}

/// What the installed sampler decided for this span, read off the SDK span it
/// started; `None` where the trace layer holds none.
fn sdk_verdict(span: &tracing::Span) -> Option<bool> {
    let context = span.context();
    let span_context = context.span().span_context().clone();
    span_context.is_valid().then(|| span_context.is_sampled())
}

/// What the sampler decides for the trace `correlation` names, asked as the SDK
/// asks it of a span, under the same parent; with none handed over, the flag
/// the correlation already carries.
fn decided(correlation: &Correlation) -> bool {
    let Some(sampler) = SAMPLER.get() else {
        return correlation.flags().is_sampled();
    };
    let parent = parent_context(
        correlation,
        TraceFlags::new(correlation.flags().bits()),
        correlation.parent_is_remote(),
    )
    .unwrap_or_default();
    let result = sampler.should_sample(
        Some(&parent),
        TraceId::from_bytes(correlation.trace_id().to_bytes()),
        "",
        &SpanKind::Internal,
        &[],
        &[],
    );
    result.decision == SamplingDecision::RecordAndSample
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use nest_rs_core::{Correlation, TraceParent, TraceState};
    use tracing::field::{Field, Visit};
    use tracing_opentelemetry::SetParentError;
    use tracing_subscriber::layer::{Context, SubscriberExt as _};

    /// The messages of the `warn` lines filed on this crate's target.
    #[derive(Clone, Default)]
    struct Warnings(Arc<Mutex<Vec<String>>>);

    struct Message<'a>(&'a mut String);

    impl Visit for Message<'_> {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                *self.0 = format!("{value:?}");
            }
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Warnings {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            if *event.metadata().level() == tracing::Level::WARN
                && event.metadata().target() == crate::TARGET
            {
                let mut message = String::new();
                event.record(&mut Message(&mut message));
                self.0.lock().unwrap().push(message);
            }
        }
    }

    /// What this crate warned while `run` ran under a subscriber without the
    /// OpenTelemetry layer.
    fn warned_without_the_layer(run: impl FnOnce()) -> Vec<String> {
        let warnings = Warnings::default();
        let subscriber = tracing_subscriber::registry().with(warnings.clone());
        tracing::subscriber::with_default(subscriber, run);
        warnings.0.lock().unwrap().clone()
    }

    /// A trace continued from the specification's own example.
    fn continued() -> Correlation {
        let parent = TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .expect("the spec's own example");
        Correlation::continued(parent, TraceState::default(), None)
    }

    /// A filtered-out span is the filter working; the other two refusals mean
    /// no continued trace keeps its parent.
    #[test]
    fn only_a_wiring_refusal_is_reported() {
        assert!(!super::is_structural(&SetParentError::SpanDisabled));
        assert!(super::is_structural(&SetParentError::LayerNotFound));
        assert!(super::is_structural(&SetParentError::AlreadyStarted));
    }

    /// A subscriber without the OpenTelemetry layer cannot take the link at all,
    /// so every continued trace would export parentless: said at `warn`, once.
    #[test]
    fn a_link_the_subscriber_cannot_take_is_reported() {
        let correlation = continued();
        let warned = warned_without_the_layer(|| {
            let span = tracing::info_span!("unit");
            super::link_remote_parent(&span, &correlation);
            super::link_remote_parent(&span, &correlation);
        });

        let expected = ["remote parent not linked; continued traces export without their parent"];
        assert_eq!(warned, expected);
    }

    /// Where no trace exports, `init` lays no OpenTelemetry layer, and a
    /// continued unit asks for no link it has nowhere to take: nothing said.
    #[test]
    fn where_no_trace_exports_a_continued_unit_asks_for_no_link() {
        super::install(false);
        let correlation = continued();
        let warned = warned_without_the_layer(|| drop(nestrs_unit_probe::open(&correlation)));

        assert_eq!(warned, Vec::<String>::new());
    }

    /// The description reaches the export as the span's status, and the class
    /// recorded after it, as an edge records its outcome once its failure line
    /// is filed, leaves it whole.
    #[test]
    fn a_failed_units_cause_is_its_exported_status_description() {
        use opentelemetry::trace::{Status, TracerProvider as _};
        use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
        use tracing_subscriber::layer::SubscriberExt as _;

        super::install(true);
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("units")));
        tracing::subscriber::with_default(subscriber, || {
            let span = nestrs_unit_probe::open(&Correlation::minted(None));
            nest_rs_core::operation_log::record_cause(&span, "the store refused the write");
            nest_rs_core::operation_log::record_error(&span, "500");
        });

        let exported = exporter.get_finished_spans().expect("the spans exported");
        let [exported] = exported.as_slice() else {
            panic!("one span exported: {exported:#?}");
        };
        assert_eq!(
            exported.status,
            Status::error("the store refused the write")
        );
    }

    /// What a unit's span exports, run through `record` under an SDK exporting
    /// to memory.
    fn exported_status(record: impl FnOnce(&tracing::Span)) -> opentelemetry::trace::Status {
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
        use tracing_subscriber::layer::SubscriberExt as _;

        super::install(true);
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("units")));
        tracing::subscriber::with_default(subscriber, || {
            record(&nestrs_unit_probe::open(&Correlation::minted(None)));
        });

        let exported = exporter.get_finished_spans().expect("the spans exported");
        let [exported] = exported.as_slice() else {
            panic!("one span exported: {exported:#?}");
        };
        exported.status.clone()
    }

    /// A unit given several causes before it failed exports one: the last, in
    /// whichever order their words sort.
    #[test]
    fn a_unit_failed_with_several_causes_exports_the_last() {
        use opentelemetry::trace::Status;

        for (first, last) in [
            (
                "the store refused the write",
                "a replica dropped the connection",
            ),
            (
                "a replica dropped the connection",
                "the store refused the write",
            ),
        ] {
            let status = exported_status(|span| {
                nest_rs_core::operation_log::record_cause(span, first);
                nest_rs_core::operation_log::record_cause(span, last);
                nest_rs_core::operation_log::record_error(span, "500");
            });
            assert_eq!(status, Status::error(last), "{first} then {last}");
        }
    }

    /// A cause kept for a failure its unit answered past describes nothing: a
    /// unit filing `ok` exports no status, one stopped first its class alone.
    #[test]
    fn a_cause_its_unit_answered_past_is_never_exported() {
        use opentelemetry::trace::Status;

        let answered = exported_status(|span| {
            nest_rs_core::operation_log::record_cause(span, "the store refused the write");
            nestrs_unit_probe::settle(span, std::time::Instant::now());
        });
        assert_eq!(answered, Status::Unset);

        let stopped = exported_status(|span| {
            nest_rs_core::operation_log::record_cause(span, "the store refused the write");
            nest_rs_core::operation_log::record_outcome(
                &nestrs_unit_probe::WORK,
                span,
                nest_rs_core::operation_log::CANCELLED,
            );
        });
        assert_eq!(stopped, Status::error(""));
    }

    /// Fails the day tracing-opentelemetry no longer fails the span an `error`
    /// event is filed in: a unit whose line files `ok` exports `Error` all the
    /// same, a failure line it filed for a chain it then answered past
    /// included.
    #[test]
    fn upstream_tracing_opentelemetry_still_fails_the_span_an_error_event_is_filed_in() {
        use opentelemetry::trace::Status;

        let status = exported_status(|span| {
            span.in_scope(|| tracing::error!(target: "probe", error = "refused", "probe failed"));
            nestrs_unit_probe::settle(span, std::time::Instant::now());
        });
        assert_eq!(
            status,
            Status::error(""),
            "tracing-opentelemetry no longer fails the span an `error` event is filed in: \
             close .claude/upstream/tracing-opentelemetry.md's entry"
        );
    }

    #[test]
    fn a_restarted_trace_has_no_parent_to_link() {
        assert_eq!(Correlation::minted(None).parent_id(), None);
    }

    #[test]
    fn a_continued_trace_links_the_callers_span() {
        let parent = TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .expect("the spec's own example");
        let correlation = Correlation::continued(parent, TraceState::default(), None);

        assert_eq!(
            correlation.parent_id().map(|id| id.to_hex()),
            Some(String::from("00f067aa0ba902b7")),
        );
        assert_eq!(
            correlation.trace_id().to_hex(),
            "4bf92f3577b34da6a3ce929d0e0e4736",
        );
    }
}
