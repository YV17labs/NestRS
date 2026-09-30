//! One structured event per request, on `nest_rs::operation`.
//!
//! The file keeps its name: it implements `NESTRS_HTTP__ACCESS_LOG`, and an
//! access log is precisely what one edge's per-request line is. What the rename
//! took away is the *family's* target wearing this edge's word — see
//! [`nest_rs_core::operation_log::TARGET`](nest_rs_core::operation_log::TARGET).
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
//!
//! # A request dropped before it answers
//!
//! A handler still running when the shutdown window closes is dropped where it
//! waits — poem closes its connection, and hyper drops the request with it — and
//! so is one whose client goes away first. That request is still a unit of work
//! the edge accepted, so it still files its line: [`Unanswered`] holds the line
//! while the request runs and files it [`CANCELLED`] if the request is dropped
//! before a response exists, with the duration it ran for. It carries no
//! `status` and no `bytes`, because nothing was answered and nothing was
//! written — the `outcome` is how the line says so, in the word every other
//! edge files a stopped unit with. A handler that panics unwinds through the
//! same guard and takes its connection down: that line says
//! [`PANIC`](nest_rs_core::operation_log::PANIC) instead.
//!
//! A *streaming* response cut mid-flow is not that case: its head was answered,
//! so its line is filed as the body ends — at the cut — with the head's status
//! and the bytes written before it. The status is what the client received, and
//! the count is what says the body was shorter than it would have been.
//!
//! [`CANCELLED`]: nest_rs_core::operation_log::CANCELLED

use std::time::Instant;

use nest_rs_core::RequestContinuation;
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
            name: crate::unit::REQUEST,
            target: nest_rs_core::operation_log::TARGET,
            message = crate::unit::REQUEST,
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
        );
    }

    /// File a request whose response this edge never got to hold — an `Err` on
    /// its way to a layer outside, which will render it. No body passed through
    /// here, so there is nothing to count; the status is what that error will
    /// answer with.
    pub(crate) fn abandoned(self, status: u16) {
        self.emit(status, 0);
    }

    /// File a request that ended before it answered — dropped
    /// ([`CANCELLED`](nest_rs_core::operation_log::CANCELLED)) or unwinding
    /// ([`PANIC`](nest_rs_core::operation_log::PANIC)), see the module doc. No
    /// `status` and no `bytes`: neither exists, and a `0` in either would be a
    /// claim about a response nobody sent.
    fn unanswered(self, outcome: &'static str) {
        tracing::info!(
            name: crate::unit::REQUEST,
            target: nest_rs_core::operation_log::TARGET,
            message = crate::unit::REQUEST,
            method = %self.method,
            path = self.uri.path(),
            outcome,
            duration_ms = nest_rs_core::operation_log::duration_ms(self.start),
            client_ip = %self.client.ip,
            forwarded = self.client.forwarded,
            user_agent = self.user_agent.as_deref(),
        );
    }
}

/// A request's line, held while the request runs and filed
/// [`CANCELLED`](nest_rs_core::operation_log::CANCELLED) if the request is
/// dropped before it answers — [`PANIC`](nest_rs_core::operation_log::PANIC)
/// if it unwinds instead.
///
/// Borrowing the continuation rather than cloning it costs the path nothing: it
/// is declared after the continuation, so a dropped request drops it first, and
/// the line is filed inside the request's own context — the same ids every other
/// line of that request carries.
pub(crate) struct Unanswered<'a> {
    log: Option<AccessLog>,
    continuation: &'a RequestContinuation,
}

impl<'a> Unanswered<'a> {
    pub(crate) fn hold(log: Option<AccessLog>, continuation: &'a RequestContinuation) -> Self {
        Self { log, continuation }
    }

    /// The request produced an answer: its line is the answer's to file.
    pub(crate) fn answered(mut self) -> Option<AccessLog> {
        self.log.take()
    }
}

impl Drop for Unanswered<'_> {
    fn drop(&mut self) {
        if let Some(log) = self.log.take() {
            // Unwinding rather than dropped: a handler that panicked takes its
            // connection down with it, and the unit still ended in a panic.
            let outcome = if std::thread::panicking() {
                nest_rs_core::operation_log::PANIC
            } else {
                nest_rs_core::operation_log::CANCELLED
            };
            self.continuation.enter(|| log.unanswered(outcome));
        }
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::Correlation;
    use nest_rs_testing::LogCapture;

    use super::*;

    fn held() -> (AccessLog, RequestContinuation) {
        let req = Request::builder().uri_str("/reports").finish();
        let log = AccessLog::open(&req, ClientIp::unknown(), Some("curl/8"));
        let continuation = RequestContinuation::new(None, Correlation::minted(None));
        (log, continuation)
    }

    fn the_line(logs: &LogCapture) -> nest_rs_testing::CapturedEvent {
        logs.expect_one(nest_rs_core::operation_log::TARGET, crate::unit::REQUEST)
    }

    /// A request dropped before it answered files its line once, `cancelled`,
    /// with no `status` and no `bytes`, in its own trace.
    #[test]
    fn a_request_dropped_unanswered_files_its_line_cancelled() {
        let logs = LogCapture::install();
        let (log, continuation) = held();
        drop(Unanswered::hold(Some(log), &continuation));

        let line = the_line(&logs);
        assert_eq!(
            line.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::CANCELLED)
        );
        assert_eq!(line.field("status"), None);
        assert_eq!(line.field("bytes"), None);
        assert_eq!(line.field("path").as_deref(), Some("/reports"));
        assert!(line.trace_id.is_some(), "{line:#?}");
    }

    /// An answered request's line is the answer's to file; the guard files none.
    #[test]
    fn an_answered_request_is_not_filed_by_the_guard() {
        let logs = LogCapture::install();
        let (log, continuation) = held();
        let answered = Unanswered::hold(Some(log), &continuation).answered();
        assert!(answered.is_some());
        logs.expect_none(nest_rs_core::operation_log::TARGET, crate::unit::REQUEST);
    }
}
