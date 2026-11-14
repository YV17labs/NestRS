//! The SDK keeps the framework's verdict on a trace the framework started
//! unsampled — a health probe's — and asks the configured sampler about every
//! other: `OTEL_TRACES_SAMPLER` still decides each trace a request starts or
//! continues.
//!
//! `nest_rs_core`'s [`operation_span!`] says, for the instant a span is
//! created, whether it starts such a trace, and this reads it back.
//!
//! [`operation_span!`]: nest_rs_core::operation_span

use opentelemetry::trace::{Link, SpanKind, TraceId, TraceState};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::trace::{SamplingDecision, SamplingResult, ShouldSample};

/// Drops a trace the framework started unsampled, and hands every other span
/// to the sampler it wraps.
#[derive(Clone, Debug)]
pub(crate) struct AdoptUnsampledStarts<S>(pub(crate) S);

impl<S: ShouldSample + Clone + 'static> ShouldSample for AdoptUnsampledStarts<S> {
    fn should_sample(
        &self,
        parent_context: Option<&Context>,
        trace_id: TraceId,
        name: &str,
        span_kind: &SpanKind,
        attributes: &[KeyValue],
        links: &[Link],
    ) -> SamplingResult {
        if nest_rs_core::__private::pending_starts_unsampled() {
            return SamplingResult {
                decision: SamplingDecision::Drop,
                attributes: Vec::new(),
                trace_state: TraceState::default(),
            };
        }
        self.0
            .should_sample(parent_context, trace_id, name, span_kind, attributes, links)
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::Correlation;
    use opentelemetry_sdk::trace::Sampler;

    use super::*;

    fn decided(correlation: &Correlation) -> SamplingDecision {
        let sampler = AdoptUnsampledStarts(Sampler::AlwaysOn);
        nest_rs_core::__private::with_pending_ids(correlation, || {
            sampler
                .should_sample(
                    None,
                    TraceId::from_bytes(correlation.trace_id().to_bytes()),
                    "unit",
                    &SpanKind::Server,
                    &[],
                    &[],
                )
                .decision
        })
    }

    #[test]
    fn a_trace_the_framework_started_unsampled_is_dropped() {
        assert_eq!(
            decided(&nest_rs_core::__private::minted_unsampled()),
            SamplingDecision::Drop,
        );
    }

    #[test]
    fn every_other_span_is_the_configured_samplers() {
        let started = Correlation::minted(None);
        let continued = Correlation::continued(
            nest_rs_core::TraceParent::parse(
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00",
            )
            .expect("the specification's example, unsampled"),
            nest_rs_core::TraceState::default(),
            None,
        );
        for correlation in [started, continued] {
            assert_eq!(decided(&correlation), SamplingDecision::RecordAndSample);
        }
    }
}
