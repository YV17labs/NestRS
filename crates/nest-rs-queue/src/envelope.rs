//! The wire envelope every backend wraps a job with — and what carries the W3C
//! trace context across the process boundary.
//!
//! ```json
//! {
//!   "v": 1,
//!   "payload": { "…the developer's payload…": true },
//!   "traceparent": "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
//!   "tracestate": "rojo=00f067aa0ba902b7",
//!   "actor_id": "…"
//! }
//! ```
//!
//! `traceparent` is the standard's own field, verbatim, because this is a
//! *context propagation* boundary and the standard exists for exactly this: the
//! producer's span becomes the job's parent, so a worker running minutes later
//! in another binary appears in the trace of the request that enqueued it —
//! under it, not merely beside it.
//!
//! `actor_id` is not W3C's, and that is deliberate rather than an omission. W3C
//! Baggage is the standard for carrying arbitrary key-value context, and it is
//! an **HTTP header** specification; this envelope is nestrs' own format, where
//! an explicit key is the honest spelling. The day the framework propagates
//! context over outbound HTTP, that is Baggage's job and this key does not
//! become one.
//!
//! # Why the id travels here and nowhere else
//!
//! Every other propagation in the framework crosses a *task*: the operation span
//! and the ambient context ride a task-local into whatever the edge spawned. A
//! queue does not. The producer and the consumer are two processes, usually two
//! deployments, and the only thing that reaches from one to the other is the
//! payload itself.
//!
//! That is what makes "one trace, end to end" true rather than aspirational: an
//! HTTP request that enqueues a job, and the worker that runs it minutes later
//! in another binary, file their events under the same `trace_id` — the job's
//! span a child of the enqueue's. No collector and no shared process: the
//! `traceparent` this envelope carries is the whole mechanism.
//!
//! # One envelope, not two
//!
//! The versioned envelope already existed — it is what lets a rolling deploy
//! fail closed rather than misinterpret bytes. The id is a **third key on it**,
//! not a second wrapper around it: nesting one envelope inside another gives two
//! things to strip, in an order every backend would have to get right, and the
//! first backend to strip them in the wrong order hands the handler a fragment
//! of its own payload.
//!
//! Building it lives here rather than in each backend for the same reason: the
//! shape is documented by this crate, so it is sealed by this crate.
//!
//! # A bare value is normal, not an error
//!
//! A queue is shared infrastructure. A job may predate a deploy, or come from a
//! system that is not this framework at all. An unversioned value is passed
//! through untouched — the consumer decodes it as a legacy raw payload, and
//! mints an id for it. Refusing it would dead-letter a perfectly good job over
//! an observability field.

use nest_rs_core::{Correlation, TraceParent, TraceState};
use serde_json::{Value, json};

use crate::inventory::WIRE_FORMAT_VERSION;

/// The envelope's version key.
const VERSION: &str = "v";
/// Where the developer's payload lives.
const PAYLOAD: &str = "payload";
/// The producer's W3C trace context — its span becomes the job's parent.
const TRACEPARENT: &str = "traceparent";
/// Vendor state, forwarded untouched because the specification requires every
/// participant to forward it, including the ones that understand none of it.
const TRACESTATE: &str = "tracestate";
/// Who the enqueue was being served for, when anyone was. Absent for a job
/// enqueued by a scheduled tick, a boot task, or an anonymous caller — and
/// absent is the answer, not a missing one.
const ACTOR_ID: &str = "actor_id";

/// Wrap a serialized payload in the wire envelope, stamped with the ambient
/// correlation id.
///
/// Called by every backend's push. With no ambient id — a job enqueued from
/// `main`, or from a path no edge opened — one is minted: the producer's own
/// events and the consumer's still group together, which is strictly more than
/// nothing.
pub fn seal(payload: Value) -> Value {
    // One read for every key: they are one value on the ambient context for the
    // reason `Correlation`'s own doc gives — an edge that carried the trace while
    // dropping the actor reopens exactly the gap this crossing closes.
    let correlation = nest_rs_core::current_correlation().unwrap_or_else(Correlation::mint);
    let mut sealed = json!({
        VERSION: WIRE_FORMAT_VERSION,
        PAYLOAD: payload,
        // **This** span is what the job's parent is, which is what makes the job
        // a child of the enqueue rather than a sibling of it.
        TRACEPARENT: correlation.traceparent().to_string(),
    });
    if let Some(tracestate) = correlation.tracestate().as_str() {
        sealed[TRACESTATE] = Value::String(tracestate.to_owned());
    }
    // The actor travels too, and it has to: a worker never re-authenticates —
    // there is no credential on a job — so if the enqueue does not say who it
    // was for, nothing downstream can ever answer it. Absent when the enqueue
    // itself had no actor, which a scheduled tick and an anonymous caller both
    // legitimately are.
    if let Some(actor_id) = correlation.actor_id() {
        sealed[ACTOR_ID] = Value::String(actor_id.to_owned());
    }
    sealed
}

/// Take the trace context off a queued value, leaving exactly the envelope the
/// job handler already knows how to decode.
///
/// `None` means the value carried no usable context — a legacy payload, a
/// foreign producer, or an envelope whose `traceparent` is malformed. The caller
/// starts a trace.
///
/// The producer is trusted here, and only here: this envelope was written by
/// *our own* enqueue into infrastructure the deployment owns. That is what makes
/// continuing sound where an arbitrary HTTP caller's header is not.
///
/// The actor rides along and is **inherited, never re-derived**: a worker holds
/// no credential and cannot authenticate anyone, so what the enqueue knew is the
/// only answer there will ever be.
pub fn open(mut value: Value) -> (Value, Option<Correlation>) {
    let Some(map) = value.as_object_mut() else {
        return (value, None);
    };
    // Removed rather than left in place: the handler decodes `payload` against
    // the developer's type, and a stray key is the framework leaking its own
    // metadata into a shape the developer declared.
    let parent = map
        .remove(TRACEPARENT)
        .as_ref()
        .and_then(Value::as_str)
        .and_then(TraceParent::parse);
    let tracestate = map
        .remove(TRACESTATE)
        .as_ref()
        .and_then(Value::as_str)
        .map(TraceState::adopt)
        .unwrap_or_default();
    let actor_id = map.remove(ACTOR_ID);
    let correlation = parent.map(|parent| {
        Correlation::continued(
            parent,
            tracestate,
            actor_id.as_ref().and_then(Value::as_str),
        )
    });
    (value, correlation)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the handler decodes must be byte-identical to the envelope it
    /// decoded before the id existed — the id rides along, it does not reshape
    /// the payload.
    #[test]
    fn opening_leaves_exactly_the_versioned_envelope() {
        let payload = json!({ "file": "song.wav" });
        let (opened, id) = open(seal(payload.clone()));
        assert_eq!(
            opened,
            json!({ VERSION: WIRE_FORMAT_VERSION, PAYLOAD: payload }),
        );
        assert!(id.is_some(), "and the id travelled with it");
    }

    fn under<F: std::future::Future>(
        correlation: Correlation,
        fut: F,
    ) -> impl std::future::Future<Output = F::Output> {
        // No scope: nothing under test resolves a provider, and the installer's
        // `Option` is what lets a caller say so instead of building an empty
        // container to satisfy a signature.
        nest_rs_core::with_request_scope(None, correlation, fut)
    }

    /// The whole point of the boundary crossing: the consumer runs inside the
    /// producer's trace, and **under** its span rather than beside it. The
    /// parent link is what a flat id could never carry across a process.
    #[tokio::test]
    async fn the_job_is_a_child_of_the_enqueue_in_the_same_trace() {
        let producer = Correlation::mint();
        let sealed = under(producer.clone(), async { seal(json!({ "clip": 1 })) }).await;

        let (_, adopted) = open(sealed);
        let adopted = adopted.expect("the context travelled");
        assert_eq!(adopted.trace_id(), producer.trace_id(), "one trace");
        assert_eq!(
            adopted.parent_id(),
            Some(producer.span_id()),
            "the job names the enqueue that caused it",
        );
        assert_ne!(
            adopted.span_id(),
            producer.span_id(),
            "and is its own unit of work",
        );
    }

    /// Vendor state is a MUST to forward, and its breakage is invisible to us
    /// and fatal to whichever vendor's sampling or routing rides in it.
    #[tokio::test]
    async fn tracestate_crosses_the_process_boundary_verbatim() {
        let parent = nest_rs_core::TraceParent::parse(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )
        .expect("the spec's own example");
        let producer = Correlation::continued(
            parent,
            nest_rs_core::TraceState::adopt("rojo=00f067aa0ba902b7,congo=t61rcWkgMzE"),
            None,
        );

        let sealed = under(producer, async { seal(json!({ "clip": 1 })) }).await;
        let (_, adopted) = open(sealed);

        assert_eq!(
            adopted.expect("context travelled").tracestate().as_str(),
            Some("rojo=00f067aa0ba902b7,congo=t61rcWkgMzE"),
        );
    }

    /// A worker holds no credential, so the actor is only ever what the enqueue
    /// knew. Without it crossing here, every job would be attributed to nobody.
    #[tokio::test]
    async fn the_actor_crosses_with_the_id() {
        let producer = Correlation::mint();
        let sealed = under(producer.clone(), async {
            nest_rs_core::set_actor_id("alice-42");
            seal(json!({ "clip": 1 }))
        })
        .await;

        let (_, adopted) = open(sealed);
        assert_eq!(
            adopted.as_ref().and_then(Correlation::actor_id),
            Some("alice-42")
        );
    }

    /// An enqueue nobody was authenticated for — a scheduled tick, an anonymous
    /// caller. The job still correlates; it is simply attributed to no one, and
    /// `None` says exactly that.
    #[tokio::test]
    async fn an_anonymous_enqueue_carries_an_id_and_no_actor() {
        let sealed = under(Correlation::mint(), async { seal(json!({ "clip": 1 })) }).await;

        let (_, adopted) = open(sealed);
        assert!(adopted.is_some(), "the job is still correlated");
        assert_eq!(
            adopted.as_ref().and_then(Correlation::actor_id),
            None,
            "and anonymous is reported as absent, never as a sentinel string",
        );
    }

    /// A queue is shared infrastructure — a value nobody stamped is passed
    /// through for the legacy decode path, not refused.
    #[test]
    fn an_unstamped_value_passes_through_untouched() {
        for bare in [
            json!({ VERSION: 1, PAYLOAD: { "clip": 1 } }),
            json!({ "clip": 1 }),
            json!("just a string"),
            json!(42),
            json!(null),
        ] {
            let (opened, id) = open(bare.clone());
            assert_eq!(opened, bare, "nothing is reshaped");
            assert!(id.is_none(), "and there is no id to adopt");
        }
    }

    /// A malformed context costs the correlation, never the job.
    #[test]
    fn an_unusable_context_is_dropped_and_the_envelope_survives() {
        let (opened, id) = open(json!({
            VERSION: 1,
            PAYLOAD: { "clip": 1 },
            TRACEPARENT: "not-a-traceparent",
        }));
        assert_eq!(opened, json!({ VERSION: 1, PAYLOAD: { "clip": 1 } }));
        assert!(id.is_none());
    }
}
