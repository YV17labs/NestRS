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

use std::sync::Once;

use opentelemetry::Context;
use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId};
use tracing_opentelemetry::{OpenTelemetrySpanExt, SetParentError};

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
    if let Err(error) = span.set_parent(Context::new().with_remote_span_context(remote))
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

/// Whether a refused link says something about the wiring rather than the span.
///
/// A span the OpenTelemetry layer's own filter disabled is not exported, so it
/// has no parent to miss — that is the filter working. The other refusals (the
/// layer is not where the span lives, the span started before the link) hold for
/// every span the process opens, which is why they are reported once rather than
/// per span.
fn is_structural(error: &SetParentError) -> bool {
    !matches!(error, SetParentError::SpanDisabled)
}

/// What the installed sampler decided for this span.
fn is_sampled(span: &tracing::Span) -> bool {
    span.context().span().span_context().is_sampled()
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{Correlation, TraceParent, TraceState};
    use tracing_opentelemetry::SetParentError;

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
        use std::sync::{Arc, Mutex};
        use tracing::field::{Field, Visit};
        use tracing_subscriber::layer::{Context, SubscriberExt};

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

        let warnings = Warnings::default();
        let subscriber = tracing_subscriber::registry().with(warnings.clone());
        let parent = TraceParent::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .expect("the spec's own example");
        let correlation = Correlation::continued(parent, TraceState::default(), None);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("unit");
            super::link_remote_parent(&span, &correlation);
            super::link_remote_parent(&span, &correlation);
        });

        let expected = ["remote parent not linked; continued traces export without their parent"];
        assert_eq!(*warnings.0.lock().unwrap(), expected);
    }

    /// A restarted trace has nothing to link, and this is what keeps that true:
    /// linking would reconnect the span to the trace a trust gate deliberately
    /// refused to join.
    #[test]
    fn a_restarted_trace_has_no_parent_to_link() {
        assert_eq!(Correlation::minted(None).parent_id(), None);
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
