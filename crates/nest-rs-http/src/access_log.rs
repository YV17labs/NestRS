//! One structured event per request, on `nest_rs::operation`.
//!
//! `trace_id` and `span_id` are not fields: every line carries its unit's
//! correlation from the ambient context, which the body re-enters
//! (`response_body`).
//!
//! A request dropped before it answers — by the shutdown window or a client's
//! reset — files its line [`CANCELLED`] through [`Unanswered`], with no `status`
//! and no `bytes`; one that panics files
//! [`PANIC`](nest_rs_core::operation_log::PANIC). An HTTP/1.1 half-close is
//! not an abandonment: hyper runs the handler to its end.
//!
//! [`CANCELLED`]: nest_rs_core::operation_log::CANCELLED

use std::time::Instant;

use nest_rs_core::RequestContinuation;
use poem::Request;
use poem::http::{Method, Uri};

use crate::matched::MatchedRoute;

/// A request being timed. Opened before the inner tree runs, filed once the
/// response body has been written.
pub(crate) struct AccessLog {
    method: Method,
    /// Cloned rather than formatted: a refcount bump on `Uri`'s `Bytes`.
    uri: Uri,
    user_agent: Option<String>,
    start: Instant,
}

impl AccessLog {
    /// Snapshot what only the *request* can answer, before it is consumed.
    /// `user_agent` is the one the operation span read.
    pub(crate) fn open(req: &Request, user_agent: Option<&str>) -> Self {
        Self {
            method: req.method().clone(),
            uri: req.uri().clone(),
            // Owned: the `HeaderValue`'s bytes are a slice of hyper's shared read
            // buffer, which holding would pin until the response body ends.
            user_agent: user_agent.map(str::to_owned),
            start: Instant::now(),
        }
    }

    /// The line. `bytes` is the response body actually written.
    ///
    /// An answer that ran to its end carries no `outcome`: its `status` says it.
    /// A body the transport stopped carries
    /// [`CANCELLED`](nest_rs_core::operation_log::CANCELLED).
    pub(crate) fn emit(
        self,
        span: &tracing::Span,
        status: u16,
        bytes: u64,
        outcome: Option<&'static str>,
    ) {
        nest_rs_core::operation_line!(
            crate::unit::REQUEST,
            span: span,
            outcome: outcome,
            started: self.start,
            method = %self.method,
            path = self.uri.path(),
            status,
            bytes,
            user_agent = self.user_agent.as_deref(),
        );
    }

    /// File a request that ended before it answered. No `status` and no
    /// `bytes`: a `0` in either would claim a response nobody sent.
    fn unanswered(self, span: &tracing::Span, outcome: &'static str) {
        nest_rs_core::operation_line!(
            crate::unit::REQUEST,
            span: span,
            outcome: outcome,
            started: self.start,
            method = %self.method,
            path = self.uri.path(),
            user_agent = self.user_agent.as_deref(),
        );
    }
}

/// What a request owes if it ends before it answers, held while it runs: its
/// span named for the route it matched and recorded failed, and its line filed
/// [`CANCELLED`](nest_rs_core::operation_log::CANCELLED) — or
/// [`PANIC`](nest_rs_core::operation_log::PANIC) if it unwinds instead.
///
/// Declared after everything it borrows, so a dropped request drops it first.
pub(crate) struct Unanswered<'a> {
    log: Option<AccessLog>,
    continuation: &'a RequestContinuation,
    span: &'a tracing::Span,
    method: &'a Method,
    matched: &'a MatchedRoute,
    answered: bool,
}

impl<'a> Unanswered<'a> {
    pub(crate) fn hold(
        log: Option<AccessLog>,
        continuation: &'a RequestContinuation,
        span: &'a tracing::Span,
        method: &'a Method,
        matched: &'a MatchedRoute,
    ) -> Self {
        Self {
            log,
            continuation,
            span,
            method,
            matched,
            answered: false,
        }
    }

    /// The request produced an answer: its span is named off the answer, and its
    /// line is the answer's to file.
    pub(crate) fn answered(mut self) -> Option<AccessLog> {
        self.answered = true;
        self.log.take()
    }
}

impl Drop for Unanswered<'_> {
    fn drop(&mut self) {
        if self.answered {
            return;
        }
        let outcome = if std::thread::panicking() {
            nest_rs_core::operation_log::PANIC
        } else {
            nest_rs_core::operation_log::CANCELLED
        };
        crate::trace_context::name_route(self.span, self.method, self.matched.route().as_deref());
        match self.log.take() {
            Some(log) => self
                .continuation
                .enter(|| log.unanswered(self.span, outcome)),
            None => nest_rs_core::operation_log::record_outcome(self.span, outcome),
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
        let log = AccessLog::open(&req, Some("curl/8"));
        let continuation = RequestContinuation::new(None, Correlation::minted(None));
        (log, continuation)
    }

    fn the_line(logs: &LogCapture) -> nest_rs_testing::CapturedEvent {
        logs.expect_one(
            nest_rs_core::operation_log::TARGET,
            crate::unit::REQUEST.name(),
        )
    }

    #[test]
    fn a_request_dropped_unanswered_files_its_line_cancelled() {
        let logs = LogCapture::install();
        let (log, continuation) = held();
        let span = tracing::Span::none();
        drop(Unanswered::hold(
            Some(log),
            &continuation,
            &span,
            &Method::GET,
            &MatchedRoute::default(),
        ));

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

    #[test]
    fn an_answered_request_is_not_filed_by_the_guard() {
        let logs = LogCapture::install();
        let (log, continuation) = held();
        let span = tracing::Span::none();
        let matched = MatchedRoute::default();
        let answered =
            Unanswered::hold(Some(log), &continuation, &span, &Method::GET, &matched).answered();
        assert!(answered.is_some());
        logs.expect_none(
            nest_rs_core::operation_log::TARGET,
            crate::unit::REQUEST.name(),
        );
    }
}
