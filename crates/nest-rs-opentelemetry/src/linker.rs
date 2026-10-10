//! What this crate adds to a span the framework already opened — at **every**
//! edge, not one: every edge opens its unit of work through `operation_span!`,
//! which calls [`link_span`](nest_rs_core::__private::link_span).
//!
//! A seeded function pointer, because both steps need the `tracing::Span`
//! handle, which exists only inside the macro, in a crate that must not depend
//! on this one: `tracing_opentelemetry::OtelData` is private and an
//! `IdGenerator` never sees a span.

use std::sync::Once;

use opentelemetry::Context;
use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId};
use tracing_opentelemetry::{OpenTelemetrySpanExt, SetParentError};

use nest_rs_core::Correlation;

/// Install [`enrich`] as the framework's span linker, before any module
/// registers.
pub(crate) fn install() {
    nest_rs_core::__private::set_span_linker(enrich);
}

/// Link the remote parent, then record what the sampler decided.
///
/// The order is load-bearing: a `ParentBased` sampler reads the parent, so a
/// verdict read before the link is the verdict for a trace this span is not in.
fn enrich(span: &tracing::Span, correlation: &Correlation) {
    link_remote_parent(span, correlation);
    nest_rs_core::__private::set_sampled(correlation, is_sampled(span));
}

/// Tell the SDK this span continues one that ran elsewhere.
///
/// Only where the framework continued a trace: a **restarted** trace has no
/// parent, and inventing one would rejoin the trace a trust gate refused.
/// Whether the parent is remote is the correlation's to say: a WS message's ran
/// in this process. The link is explicit even for a local parent, since a WS
/// message and an MCP operation run on a task their parent's span never entered.
fn link_remote_parent(span: &tracing::Span, correlation: &Correlation) {
    let Some(parent_id) = correlation.parent_id() else {
        return;
    };
    let remote = SpanContext::new(
        TraceId::from_bytes(correlation.trace_id().to_bytes()),
        SpanId::from_bytes(parent_id.to_bytes()),
        TraceFlags::new(correlation.flags().bits()),
        correlation.parent_is_remote(),
        // The SDK's own `FromStr`, rather than a private splitter that would drift.
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

/// Whether a refused link says something about the wiring rather than the span:
/// a span the layer's own filter disabled has no parent to miss.
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
