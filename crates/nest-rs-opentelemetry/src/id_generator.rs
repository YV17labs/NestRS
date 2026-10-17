//! The SDK adopts the framework's ids; it never mints a second pair, which
//! would name one unit of work twice — once in the logs, once in the exported
//! trace — with nothing joining them.
//!
//! `nest_rs_core`'s [`operation_span!`] publishes the ids for the instant a span
//! is created, and this reads them back.
//!
//! [`operation_span!`]: nest_rs_core::operation_span

use opentelemetry::trace::{SpanId, TraceId};
use opentelemetry_sdk::trace::{IdGenerator, RandomIdGenerator};

/// Takes the framework's ids where the framework opened the span, and falls back
/// to the SDK's own generator for a span nobody here opened (a library's, a
/// `#[tracing::instrument]`).
#[derive(Debug, Default)]
pub(crate) struct AdoptFrameworkIds(RandomIdGenerator);

impl IdGenerator for AdoptFrameworkIds {
    fn new_trace_id(&self) -> TraceId {
        match nest_rs_core::__private::pending_ids() {
            Some((trace_id, _)) => TraceId::from_bytes(trace_id.to_bytes()),
            None => self.0.new_trace_id(),
        }
    }

    fn new_span_id(&self) -> SpanId {
        match nest_rs_core::__private::pending_ids() {
            Some((_, span_id)) => SpanId::from_bytes(span_id.to_bytes()),
            None => self.0.new_span_id(),
        }
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::Correlation;

    use super::*;

    #[test]
    fn the_frameworks_ids_are_what_the_sdk_receives() {
        let correlation = Correlation::minted(None);
        let generator = AdoptFrameworkIds::default();

        let (trace_id, span_id) = nest_rs_core::__private::with_pending_ids(&correlation, || {
            (generator.new_trace_id(), generator.new_span_id())
        });

        assert_eq!(
            trace_id.to_string(),
            correlation.trace_id().to_hex(),
            "the exported trace is the one the logs name",
        );
        assert_eq!(
            span_id.to_string(),
            correlation.span_id().to_hex(),
            "and so is the span",
        );
    }

    /// A package's unit, declared under its own crate's name, is exported under
    /// the ids its operation line carries: the proof runs the whole pipeline,
    /// from `operation_span!` through the layer to the exporter.
    #[test]
    fn a_package_units_exported_span_carries_the_ids_its_operation_line_carries() {
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
        use tracing_subscriber::layer::SubscriberExt as _;

        // As `init_with` does: the linker is what starts the SDK's span while the
        // ids are pending.
        crate::linker::install();
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_id_generator(AdoptFrameworkIds::default())
            .with_simple_exporter(exporter.clone())
            .build();
        let exporting = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("probe")));
        let logs = nest_rs_testing::LogCapture::install();
        let correlation = Correlation::minted(None);
        let started = std::time::Instant::now();

        // The capture is this thread's default; the span is opened under the
        // exporting subscriber, which it keeps until it closes.
        let span =
            tracing::subscriber::with_default(exporting, || nestrs_unit_probe::open(&correlation));
        nest_rs_core::RequestContinuation::new(None, correlation.clone())
            .enter(|| nestrs_unit_probe::settle(&span, started));
        drop(span);

        let unit = nestrs_unit_probe::WORK;
        let exported = exporter
            .get_finished_spans()
            .expect("the in-memory exporter answers");
        let [exported] = exported.as_slice() else {
            panic!("one span exported: {exported:#?}");
        };
        let line = logs.expect_one(nest_rs_core::operation_log::TARGET, unit.name());
        assert_eq!(line.name, unit.name());
        assert_eq!(exported.name, unit.name());
        assert_eq!(
            Some(exported.span_context.trace_id().to_string()),
            line.trace_id,
            "the exported trace is the one the line names",
        );
        assert_eq!(
            Some(exported.span_context.span_id().to_string()),
            line.span_id,
            "and so is the span",
        );
    }

    #[test]
    fn a_span_the_framework_did_not_open_gets_its_own_ids() {
        let generator = AdoptFrameworkIds::default();
        assert_ne!(generator.new_span_id(), SpanId::INVALID);
        assert_ne!(generator.new_span_id(), generator.new_span_id());
    }
}
