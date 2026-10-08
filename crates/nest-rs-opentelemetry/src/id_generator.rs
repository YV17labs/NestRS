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
        match nest_rs_core::trace_context::pending_ids() {
            Some((trace_id, _)) => TraceId::from_bytes(trace_id.to_bytes()),
            None => self.0.new_trace_id(),
        }
    }

    fn new_span_id(&self) -> SpanId {
        match nest_rs_core::trace_context::pending_ids() {
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

        let (trace_id, span_id) =
            nest_rs_core::trace_context::with_pending_ids(&correlation, || {
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

    #[test]
    fn a_span_the_framework_did_not_open_gets_its_own_ids() {
        let generator = AdoptFrameworkIds::default();
        assert_ne!(generator.new_span_id(), SpanId::INVALID);
        assert_ne!(generator.new_span_id(), generator.new_span_id());
    }
}
