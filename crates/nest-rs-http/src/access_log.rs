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
//! `trace_id` and `span_id` are the transport's too — W3C Trace Context, minted
//! or continued by the edge with no collector, propagator or exporter involved.
//! The observability stack *enriches* what is exported; it never decides what
//! this line can say.
//!
//! They are **not** written here, and that is deliberate: every line the console
//! renders carries the correlation of the unit of work it belongs to, read off
//! the ambient context, so spelling them as event fields would print them twice
//! in text and put them in two different positions in the JSON envelope. The
//! body re-enters the request's context around this line for exactly that
//! reason — see `response_body`.
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

use crate::client_ip::ClientIp;

/// A request being timed. Opened before the inner tree runs, filed once the
/// response body has been written.
pub(crate) struct AccessLog {
    method: Method,
    /// Cloned rather than formatted: `Uri`'s path is a `Bytes` slice, so this is
    /// a refcount bump and the path is read back without allocating.
    uri: Uri,
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
    pub(crate) fn open(req: &Request, client: ClientIp, user_agent: Option<&str>) -> Self {
        Self {
            method: req.method().clone(),
            uri: req.uri().clone(),
            client,
            // Owned rather than borrowed: the `HeaderValue`'s bytes are a slice
            // of hyper's shared read buffer, which holding would pin until the
            // response body ends.
            user_agent: user_agent.map(str::to_owned),
            start: Instant::now(),
        }
    }

    /// The line. `bytes` is the response body actually written.
    ///
    /// The target and the duration formula come from
    /// [`operation_log`](nest_rs_core::operation_log) rather than being spelled
    /// here: this is one member of a family every edge files into, and a second
    /// copy of either is what makes a family drift while both halves look right.
    ///
    /// **No `outcome` field, and that is deliberate.** Its peers carry one
    /// because they have no other way to say how the work ended; a request has
    /// `status`, which says it more precisely than three words could. Classifying
    /// a `404` or a `401` as `ok` or `error` is a judgement the framework has no
    /// business making on an operator's behalf.
    pub(crate) fn emit(self, status: u16, bytes: u64) {
        tracing::info!(
            target: nest_rs_core::operation_log::TARGET,
            method = %self.method,
            path = self.uri.path(),
            status,
            bytes,
            duration_ms = nest_rs_core::operation_log::duration_ms(self.start),
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
