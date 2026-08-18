//! What this crate adds to a span the framework already opened — at **every**
//! edge, not one.
//!
//! # Why this is not an interceptor
//!
//! It used to be one, on the HTTP transport band, and that reached exactly one
//! member of the family. A queue job continues a trace out of the wire envelope
//! and had no way to say so: its exported span landed in the right trace, with
//! the right id, and no causal edge to the enqueue that caused it — the one
//! relation the whole change exists to carry across a process. A WS message, a
//! subscription and a scheduled tick were in the same position, and none of them
//! ever had its sampler's verdict written back, so a `traceparent` they sealed
//! reported a `sampled` bit nobody decided.
//!
//! So the enrichment hangs off the **span constructor** instead. Every edge opens
//! its unit of work through `operation_span!`, which calls
//! [`link_span`](nest_rs_core::trace_context::link_span) — so a new edge inherits
//! this the day it is written, without `nest-rs-queue` or `nest-rs-ws` learning
//! that OpenTelemetry exists.
//!
//! # Why a seeded function pointer
//!
//! Both things this does need a `tracing::Span` **handle**:
//! `OpenTelemetrySpanExt::set_parent` takes one, and reading the sampling verdict
//! goes through the same extension. An `IdGenerator` never sees a span, and
//! `tracing_opentelemetry::OtelData` — what a subscriber layer would reach for —
//! is private. The handle exists in exactly one place, inside the macro, in a
//! crate that must not depend on this one. A pointer seeded at boot is how the
//! dependency runs backwards; it is the shape `WsDataPipe` already uses.

use opentelemetry::Context;
use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use nest_rs_core::Correlation;

/// Install [`enrich`] as the framework's span linker. Called once, from
/// [`OpenTelemetry::init_with`](crate::OpenTelemetry), before any module
/// registers.
pub(crate) fn install() {
    nest_rs_core::trace_context::set_span_linker(enrich);
}

/// Link the remote parent, then record what the sampler decided.
///
/// The order is load-bearing: a `ParentBased` sampler reads the parent, so a
/// verdict read before the link is the verdict for a trace this span is not in.
fn enrich(span: &tracing::Span, correlation: &Correlation) {
    link_remote_parent(span, correlation);
    correlation.set_sampled(is_sampled(span));
}

/// Tell the SDK this span continues one that ran elsewhere.
///
/// Only where the framework actually continued a trace. A **restarted** trace has
/// no parent by definition — that is what restarting means — and inventing one
/// would reconnect the span to the very trace a trust gate refused to join.
///
/// Whether the parent is **remote** is the correlation's to say, not this
/// function's: a queue job's parent ran in another process, a WS message's ran
/// in this one. Claiming remote for an in-process parent describes a network hop
/// that never happened, and a backend renders it as one.
///
/// The link is explicit even where the parent is local, because at those sites
/// there is no `tracing` nesting to infer it from — a WS message and an MCP
/// operation both run on a task their parent's span never entered.
fn link_remote_parent(span: &tracing::Span, correlation: &Correlation) {
    let Some(parent_id) = correlation.parent_id() else {
        return;
    };
    let remote = SpanContext::new(
        TraceId::from_bytes(correlation.trace_id().to_bytes()),
        SpanId::from_bytes(parent_id.to_bytes()),
        TraceFlags::new(correlation.flags().bits()),
        correlation.parent_is_remote(),
        // Parsed by the SDK's own `FromStr`: it implements the grammar already,
        // and a private splitter would drift from it exactly where the drift is
        // invisible.
        correlation
            .tracestate()
            .as_str()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or_default(),
    );
    let _ = span.set_parent(Context::new().with_remote_span_context(remote));
}

/// What the installed sampler decided for this span.
fn is_sampled(span: &tracing::Span) -> bool {
    span.context().span().span_context().is_sampled()
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{Correlation, TraceParent, TraceState};

    /// A restarted trace has nothing to link, and this is what keeps that true:
    /// linking would reconnect the span to the trace a trust gate deliberately
    /// refused to join.
    #[test]
    fn a_restarted_trace_has_no_parent_to_link() {
        assert_eq!(Correlation::mint().parent_id(), None);
    }

    /// And a continued one does — the caller's span, which is what makes the
    /// exported span a child rather than a second root in the same trace. This
    /// is the relation that was missing at every edge but HTTP.
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
