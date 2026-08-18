//! The SDK adopts the framework's ids; it never mints a second pair.
//!
//! # The duplication this exists to prevent
//!
//! An OpenTelemetry SDK generates a trace id and a span id whenever a span is
//! created. Left alone it would do that here too — and the framework already
//! decided both, in the kernel, before this crate had a chance to be installed.
//! The result would be two identifiers for one unit of work: the one on every
//! log line and in every job envelope, and the one in the exported trace, with
//! nothing joining them.
//!
//! That is precisely the duplication that retiring the framework's homemade
//! `request_id` removed. Reintroducing it one layer down would be worse, because
//! it would be invisible: both halves would look correct on their own.
//!
//! So the generator is inverted. `nest_rs_core`'s [`operation_span!`] publishes
//! the ids for the instant a span is created, and this reads them back. What the
//! backend shows and what the logs say are then the same value by construction
//! rather than by convention.
//!
//! [`operation_span!`]: nest_rs_core::operation_span

use opentelemetry::trace::{SpanId, TraceId};
use opentelemetry_sdk::trace::{IdGenerator, RandomIdGenerator};

/// Takes the framework's ids where the framework opened the span, and falls back
/// to the SDK's own generator where it did not.
///
/// The fallback is not a safety net, it is the correct answer for a span nobody
/// here opened: a library's internal span, a `#[tracing::instrument]` a developer
/// wrote inside a handler. Those are genuinely new spans and deserve new ids;
/// what they inherit — the trace — comes from their parent, as it should.
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

    /// The whole contract: inside an operation span's creation, the SDK is
    /// handed the framework's ids. Without this the exported trace and the log
    /// lines name the same request by two different identifiers.
    #[test]
    fn the_frameworks_ids_are_what_the_sdk_receives() {
        let correlation = Correlation::mint();
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

    /// A span the framework did not open is a real new span — a library's own,
    /// a `#[tracing::instrument]` inside a handler — and it gets real new ids.
    #[test]
    fn a_span_the_framework_did_not_open_gets_its_own_ids() {
        let generator = AdoptFrameworkIds::default();
        assert_ne!(generator.new_span_id(), SpanId::INVALID);
        assert_ne!(generator.new_span_id(), generator.new_span_id());
    }
}
