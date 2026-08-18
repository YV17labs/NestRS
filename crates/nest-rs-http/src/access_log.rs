//! One structured event per request, on `nest_rs::access`.
//!
//! # Why the transport owns this
//!
//! Everything the line carries — method, path, status, duration, client, user
//! agent, and the request's [`Correlation`] — is what *this* transport knows
//! about a request it served. None of it needs a collector, a propagator or an
//! exporter, so none of it may depend on one being mounted: an access log is how
//! an operator answers "what did this deployment do", and that question does not
//! become answerable only once OTLP is configured.
//!
//! `trace_id` and `span_id` are the transport's too, and that is the change
//! this shape records: they are W3C Trace Context, minted or continued by the
//! edge with no collector, propagator or exporter involved. The observability
//! stack *enriches* what is exported; it never decides what this line can say.
//!
//! # What is filed here, and what is not
//!
//! The line reports the body's size, so it is filed once the body has been
//! written — by [`response_body`](crate::response_body), which wraps the body
//! for that and for the larger reason that a streaming response is still the
//! request running. This module owns the *line*; that one owns the *body*, and
//! the split is deliberate: the body is carried whether or not the line is on.

use std::time::Instant;

use poem::Request;
use poem::http::{Method, Uri};

use nest_rs_core::Correlation;

use crate::client_ip::ClientIp;

/// A request being timed. Opened before the inner tree runs, filed once the
/// response body has been written.
pub(crate) struct AccessLog {
    method: Method,
    /// Cloned rather than formatted: `Uri`'s path is a `Bytes` slice, so this is
    /// a refcount bump and the path is read back without allocating.
    uri: Uri,
    /// The whole correlation, not just its id: the actor is resolved by a guard
    /// *after* this is opened, and the shared `OnceLock` is what lets the line
    /// report who was served without the edge having to wait for authentication
    /// to happen. A line that cannot say who was denied is the gap this change
    /// exists to close.
    correlation: Correlation,
    client: ClientIp,
    user_agent: Option<String>,
    start: Instant,
}

impl AccessLog {
    /// Snapshot what only the *request* can answer, before it is consumed.
    ///
    /// `client` and `user_agent` are passed in rather than read here because the
    /// operation span needs the same answers: reading twice would be two chances
    /// for the span and the line to disagree about who called.
    pub(crate) fn open(
        req: &Request,
        correlation: Correlation,
        client: ClientIp,
        user_agent: Option<&str>,
    ) -> Self {
        Self {
            method: req.method().clone(),
            uri: req.uri().clone(),
            correlation,
            client,
            // Owned rather than borrowed: the `HeaderValue`'s bytes are a slice
            // of hyper's shared read buffer, which holding would pin until the
            // response body ends.
            user_agent: user_agent.map(str::to_owned),
            start: Instant::now(),
        }
    }

    /// The line. `bytes` is the response body actually written.
    pub(crate) fn emit(self, status: u16, bytes: u64) {
        // Microsecond resolution, rendered in milliseconds: a sub-millisecond
        // request is the common case and `0` tells an operator nothing.
        let duration_ms = (self.start.elapsed().as_secs_f64() * 1e6).round() / 1e3;
        tracing::info!(
            target: "nest_rs::access",
            method = %self.method,
            path = self.uri.path(),
            status,
            bytes,
            duration_ms,
            trace_id = %self.correlation.trace_id(),
            span_id = %self.correlation.span_id(),
            // Absent for an anonymous caller, which is the answer rather than a
            // missing one — and present on exactly the line an operator greps
            // after a `403`.
            actor_id = self.correlation.actor_id(),
            client_ip = %self.client.ip,
            // Whether the address came from a proxy header or from the peer.
            // Two very different confidences, and an incident query that cannot
            // tell them apart is reading a number it should not trust.
            forwarded = self.client.forwarded,
            user_agent = self.user_agent.as_deref(),
            "request served",
        );
    }

    /// File a request whose response this edge never got to hold — an `Err` on
    /// its way to a layer outside, which will render it. No body passed through
    /// here, so there is nothing to count; the status is what that error will
    /// answer with.
    pub(crate) fn abandoned(self, status: u16) {
        self.emit(status, 0);
    }
}
