//! One structured event per request, on `nest_rs::operation`.
//!
//! `trace_id` and `span_id` are not fields: every line carries its unit's
//! correlation from the ambient context, which the body re-enters
//! (`response_body`). The line carries the server's values only
//! (`.claude/decisions/payload-never-quoted.md`): the route template the router
//! matched (`/posts/{id}`), never the path the client sent, and a request no
//! route matched files no route at all; the method as the HTTP conventions
//! know it, `_OTHER` for a token the client chose; no user agent, which is
//! free text.
//!
//! A request dropped before it answers — by the shutdown window or a client's
//! reset — files its line [`CANCELLED`] through [`Unanswered`], with no `status`
//! and no `bytes`. A handler that unwinds is answered `500` by the edge, and its
//! line files [`PANIC`] with that `status`, as does a body that unwinds as it is
//! written, with what it wrote. An HTTP/1.1 half-close is not an abandonment:
//! hyper runs the handler to its end.
//!
//! [`CANCELLED`]: nest_rs_core::operation_log::CANCELLED
//! [`PANIC`]: nest_rs_core::operation_log::PANIC

use std::borrow::Cow;
use std::time::Instant;

use nest_rs_core::RequestContinuation;

use crate::matched::MatchedRoute;

/// A request being timed. Opened before the inner tree runs, filed once the
/// response body has been written.
pub(crate) struct AccessLog {
    /// The [`KnownMethods`](crate::trace_context::KnownMethods) name.
    method: Cow<'static, str>,
    /// The slot the router notes the matched template in; read when the line
    /// is filed.
    matched: MatchedRoute,
    start: Instant,
}

impl AccessLog {
    /// Snapshot what only the *request* can answer, before it is consumed:
    /// its method as the line names it. `start` is the instant the request
    /// arrived, which its budget counts from too.
    pub(crate) fn open(method: Cow<'static, str>, matched: &MatchedRoute, start: Instant) -> Self {
        Self {
            method,
            matched: matched.clone(),
            start,
        }
    }

    /// The line. `bytes` is the response body actually written.
    ///
    /// An answer that ran to its end carries no `outcome`: its `status` says it.
    /// A body the transport stopped carries
    /// [`CANCELLED`](nest_rs_core::operation_log::CANCELLED), and the answer to
    /// an unwind, or a body that unwound,
    /// [`PANIC`](nest_rs_core::operation_log::PANIC).
    pub(crate) fn emit(
        self,
        span: &tracing::Span,
        status: u16,
        bytes: u64,
        outcome: Option<&'static str>,
    ) {
        let route = self.matched.route();
        nest_rs_core::operation_line!(
            crate::unit::REQUEST,
            span: span,
            outcome: outcome,
            started: self.start,
            method = %self.method,
            route = route.as_deref(),
            status,
            bytes,
        );
    }

    /// File a request that ended before it answered. No `status` and no
    /// `bytes`: a `0` in either would claim a response nobody sent.
    fn unanswered(self, span: &tracing::Span, outcome: &'static str) {
        let route = self.matched.route();
        nest_rs_core::operation_line!(
            crate::unit::REQUEST,
            span: span,
            outcome: outcome,
            started: self.start,
            method = %self.method,
            route = route.as_deref(),
        );
    }
}

/// What a request owes if it ends before it answers, held while it runs: its
/// span named for the route it matched and recorded failed, and its line filed
/// [`CANCELLED`](nest_rs_core::operation_log::CANCELLED) — or
/// [`PANIC`](nest_rs_core::operation_log::PANIC) if an unwind the edge does not
/// contain takes it down.
///
/// Declared after everything it borrows, so a dropped request drops it first.
pub(crate) struct Unanswered<'a> {
    log: Option<AccessLog>,
    continuation: &'a RequestContinuation,
    span: &'a tracing::Span,
    method: &'a str,
    matched: &'a MatchedRoute,
    answered: bool,
}

impl<'a> Unanswered<'a> {
    pub(crate) fn hold(
        log: Option<AccessLog>,
        continuation: &'a RequestContinuation,
        span: &'a tracing::Span,
        method: &'a str,
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

    /// A line opened on a request the router matched `matched` for.
    fn held(matched: &MatchedRoute) -> (AccessLog, RequestContinuation) {
        let log = AccessLog::open(Cow::Borrowed("GET"), matched, Instant::now());
        let continuation = RequestContinuation::new(None, Correlation::minted(None));
        (log, continuation)
    }

    fn matched(template: &str) -> MatchedRoute {
        let matched = MatchedRoute::default();
        matched.note(&std::sync::Arc::from(template));
        matched
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
        let route = matched("/reports/{id}");
        let (log, continuation) = held(&route);
        let span = tracing::Span::none();
        drop(Unanswered::hold(
            Some(log),
            &continuation,
            &span,
            "GET",
            &route,
        ));

        let line = the_line(&logs);
        assert_eq!(
            line.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::CANCELLED)
        );
        assert_eq!(line.field("status"), None);
        assert_eq!(line.field("bytes"), None);
        assert_eq!(line.field("route").as_deref(), Some("/reports/{id}"));
        assert!(line.trace_id.is_some(), "{line:#?}");
    }

    /// A path is what the client sent: the line names the template it
    /// matched, and a request that matched none names no route at all.
    #[test]
    fn the_line_names_the_matched_template_never_the_path_sent() {
        for (route, named) in [
            (matched("/reports/{id}"), Some("/reports/{id}")),
            (MatchedRoute::default(), None),
        ] {
            let logs = LogCapture::install();
            let (log, _continuation) = held(&route);
            log.emit(&tracing::Span::none(), 200, 2, None);
            let line = the_line(&logs);
            assert_eq!(line.field("route").as_deref(), named);
        }
    }

    /// What the console's text format is written to.
    #[derive(Clone, Default)]
    struct Written(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Written {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Written {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// The console's text format prints a field written as a `&str` in quotes
    /// and one written through `Display` bare: the method is printed
    /// `method=GET`, as a query on the console's lines matches it.
    #[test]
    fn the_console_line_prints_the_method_bare() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let written = Written::default();
        let layer = tracing_subscriber::fmt::layer()
            .event_format(nest_rs_core::__private::TextFormat::new(false))
            .with_ansi(false)
            .with_writer(written.clone());
        let subscriber = tracing_subscriber::Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let (log, _continuation) = held(&matched("/reports/{id}"));
            log.emit(&tracing::Span::none(), 200, 2, None);
        });
        let line = String::from_utf8(written.0.lock().expect("unpoisoned").clone())
            .expect("the console writes UTF-8");
        assert!(line.contains(" method=GET "), "{line}");
    }

    #[test]
    fn an_answered_request_is_not_filed_by_the_guard() {
        let logs = LogCapture::install();
        let matched = MatchedRoute::default();
        let (log, continuation) = held(&matched);
        let span = tracing::Span::none();
        let answered =
            Unanswered::hold(Some(log), &continuation, &span, "GET", &matched).answered();
        assert!(answered.is_some());
        logs.expect_none(
            nest_rs_core::operation_log::TARGET,
            crate::unit::REQUEST.name(),
        );
    }
}
