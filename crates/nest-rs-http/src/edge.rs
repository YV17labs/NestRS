//! The fused transport-edge endpoint: trailing-slash trim, request scope, the
//! body's wrapper, the request deadline and default response headers, in that
//! order, in one layer.
//!
//! The edge reads no body: it wraps one ([`Capped`]) and the extractor that
//! needs it reads it, after routing and the guards, so a refused request is
//! answered before a byte of its body is taken — and `Expect: 100-continue` is
//! never answered `100` for it, since hyper writes that on the first read.
//!
//! Every answer is stamped with the default headers, each where the response
//! left it unset: a `408`, `413` or `503` the edge produces, an `Err` escaping
//! the inner tree, which the edge renders as a problem document first, and the
//! opaque `500` it answers a handler that unwound. The normalizer for a
//! raw-text error a handler answered with runs on the edge itself when it is
//! outermost (`normalize`).

use std::any::Any;
use std::future::{Future, poll_fn};
use std::net::IpAddr;
use std::pin::pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use http::uri::{PathAndQuery, Uri};
use http::{HeaderName, HeaderValue, StatusCode, Version, header};
use http_body_util::combinators::BoxBody;
use nest_rs_core::{Container, RequestContinuation, RequestScope};
use tokio::time::Instant;

use tracing::Instrument;

use crate::access_log::{AccessLog, Unanswered};
use crate::body::Body;
use crate::capped::{Capped, Overrun, Verdict};
use crate::client_ip::ClientOrigin;
use crate::drain::Drain;
use crate::endpoint::Endpoint;
use crate::error::{HttpError, ResponseError, Result};
use crate::location::CallerUri;
use crate::matched::{self, MatchedRoute, Stamp};
use crate::problem::ProblemDetails;
use crate::request::Request;
use crate::response::{IntoResponse, Response};
use crate::{response_body, trace_context};

/// `err` as the problem document it answers, keeping the template the router
/// matched, which rendering would otherwise drop and the span is named for.
async fn rendered(err: HttpError) -> Response {
    let resp = err.answer();
    let stamp = Stamp::of(&resp);
    let mut resp = crate::problem::normalize_error_response(resp).await;
    if let Some(stamp) = stamp {
        stamp.onto(&mut resp);
    }
    resp
}

/// What a request that outran its budget is answered with: `503`, not `504`, since
/// RFC 9110 §15.6.5 scopes `504` to a gateway and this server is the origin (§15.6.4).
fn timed_out(timeout: Duration) -> Response {
    // Never `0`, which reads as "retry immediately" (RFC 9110 §10.2.3).
    let seconds = timeout
        .as_secs()
        .saturating_add(u64::from(timeout.subsec_nanos() > 0))
        .max(1);
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(header::RETRY_AFTER, HeaderValue::from(seconds))
        .finish()
}

/// What a body read that stopped is answered with: `413` past the cap, `408`
/// past the gap bound. The `408` closes an HTTP/1 connection, as RFC 9110
/// §15.5.9 asks — the unread rest leaves it unusable; HTTP/2 ends that stream
/// alone and forbids the header (RFC 9113 §8.2.2).
fn refused(overrun: Overrun, version: Version) -> Response {
    let mut resp = overrun.error().as_response();
    if matches!(overrun, Overrun::Gap(_)) && version < Version::HTTP_2 {
        resp.headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("close"));
    }
    resp
}

/// The path without its trailing slashes, or `None` when already canonical.
/// An interior `//` stays: an empty segment mid-path is a different path.
fn canonical_path(path: &str) -> Option<&str> {
    if !path.ends_with('/') || path == "/" {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    Some(if trimmed.is_empty() { "/" } else { trimmed })
}

/// Rewrite the request URI onto [`canonical_path`], query preserved. Runs before
/// [`CallerUri`] is captured, so an echoed `Location` names the canonical path.
fn trim_trailing_slash(req: &mut Request) {
    let Some(path_and_query) = req.uri().path_and_query() else {
        return;
    };
    let Some(canonical) = canonical_path(path_and_query.path()) else {
        return;
    };
    let rebuilt = match path_and_query.query() {
        Some(query) => format!("{canonical}?{query}"),
        None => canonical.to_owned(),
    };
    // Unreachable (both halves parsed already); leaving the path as sent 404s
    // rather than misroutes.
    let Ok(path_and_query) = PathAndQuery::from_str(&rebuilt) else {
        return;
    };
    // Cloned, not `mem::take`n: a failed `Uri::from_parts` would leave
    // `Uri::default()` and route the request to `/`.
    let mut parts = req.uri().clone().into_parts();
    parts.path_and_query = Some(path_and_query);
    if let Ok(uri) = Uri::from_parts(parts) {
        *req.uri_mut() = uri;
    }
}

/// The single transport-edge layer around the composed route tree; CORS and
/// compression, when configured, wrap outside it.
pub(crate) struct EdgeEndpoint<E> {
    inner: E,
    container: Container,
    /// Present when no provider is request-scoped or transient: such a scope
    /// caches nothing, so one shared instance stands in for a fresh one.
    shared_scope: Option<Arc<RequestScope>>,
    timeout: Option<Duration>,
    /// The cap a body is held to, and what its readers read
    /// ([`current_body_limit`](crate::current_body_limit)).
    body_limit: usize,
    /// The gap a body may leave between two frames once something reads it.
    body_read_timeout: Duration,
    /// Each fills only what the response left unset: a value a handler, a
    /// decorator or a surface set is its own.
    headers: Vec<(HeaderName, HeaderValue)>,
    /// The edge is outermost (no CORS or compression) and runs the problem
    /// normalizer itself.
    normalize: bool,
    /// An outer layer (compression) can replace the body under a stale
    /// `Content-Length`; otherwise hyper's length decoder already bounds it.
    counts_body: bool,
    /// Whether a request is filed; the correlation id is echoed regardless.
    access_log: bool,
    /// Decides whether `X-Forwarded-For` and `X-Request-Id` may be believed,
    /// whether or not a line is filed.
    trusted_proxies: Arc<[IpAddr]>,
    drain: Arc<Drain>,
}

impl<E> EdgeEndpoint<E> {
    #[expect(
        clippy::too_many_arguments,
        reason = "one value per decision the transport hands its edge, each a field read once \
                  per request; two call sites, both in `HttpTransport::configure`"
    )]
    pub(crate) fn new(
        inner: E,
        container: Container,
        timeout: Option<Duration>,
        body_limit: usize,
        body_read_timeout: Duration,
        headers: Vec<(HeaderName, HeaderValue)>,
        normalize: bool,
        counts_body: bool,
        drain: Arc<Drain>,
    ) -> Self {
        let shared_scope = (!container.has_dynamic_scopes())
            .then(|| Arc::new(RequestScope::new(container.clone())));
        // Read at boot: the edge runs before the request scope `ClientOrigin::of`
        // reads through. No `HttpConfig` means the defaults: line on, nothing trusted.
        let config = container.get::<crate::HttpConfig>();
        let access_log = config.as_deref().is_none_or(|config| config.access_log);
        let trusted_proxies = config.as_deref().map_or_else(
            || Arc::from([]),
            |config| config.trusted_proxies.as_slice().into(),
        );
        Self {
            inner,
            container,
            shared_scope,
            timeout,
            body_limit,
            body_read_timeout,
            headers,
            normalize,
            counts_body,
            access_log,
            trusted_proxies,
            drain,
        }
    }

    fn finish(&self, mut resp: Response) -> Response {
        let headers = resp.headers_mut();
        for (name, value) in &self.headers {
            headers.entry(name).or_insert_with(|| value.clone());
        }
        resp
    }

    /// What a request whose handler unwound is answered: the opaque `500`
    /// every `5xx` answers, the panic said once inside the request.
    fn unwound(
        &self,
        payload: &(dyn Any + Send),
        continuation: &RequestContinuation,
        span: &tracing::Span,
    ) -> Response {
        continuation.enter(|| {
            span.in_scope(|| {
                nest_rs_core::contained_panic!(
                    target: crate::target::HTTP,
                    payload,
                    "a handler panicked; answered 500",
                );
            });
        });
        self.finish(ProblemDetails::internal().into_response())
    }
}

impl<E: Endpoint> EdgeEndpoint<E> {
    /// Wrap a body that is not empty in [`Capped`], counted where its declared
    /// length cannot be trusted; the verdict the edge answers from, if it
    /// wrapped one. Nothing is read.
    fn wrap_body(&self, req: &mut Request, declared: bool) -> Option<Verdict> {
        let body = req.take_body();
        if body.is_empty() {
            req.set_body(body);
            return None;
        }
        let limit = (!declared || self.counts_body).then_some(self.body_limit);
        let verdict = Verdict::default();
        let capped = Capped::new(body, limit, self.body_read_timeout, Arc::clone(&verdict));
        req.set_body(Body::from_boxed(BoxBody::new(capped)));
        Some(verdict)
    }

    /// The inner tree, under the context `call` built and installs again
    /// around the response body, and the budget counted from `arrival`.
    async fn handle(
        &self,
        mut req: Request,
        continuation: &RequestContinuation,
        arrival: Instant,
    ) -> Response {
        // The router matches exactly: `/kitchen/` would 404 before any guard runs.
        trim_trailing_slash(&mut req);

        // The last point that sees the URI whole: the router strips a global prefix
        // off `uri()`, and `original_uri()` is set on the hyper path only.
        let caller_uri = req.uri().clone();
        req.extensions_mut().insert(CallerUri(caller_uri));

        // A declared length past the cap is refused unread. Below it, a length is
        // a claim hyper's decoder holds the body to, unless an outer layer can
        // falsify it (`counts_body`).
        let version = req.version();
        let declared = req
            .header(header::CONTENT_LENGTH)
            .and_then(|declared| declared.parse::<u64>().ok());
        if declared.is_some_and(|declared| declared > self.body_limit as u64) {
            return self.finish(refused(Overrun::Cap(self.body_limit), version));
        }
        let verdict = self.wrap_body(&mut req, declared.is_some());

        // The deadline arms on the first `Pending`: a timeout only fires at an
        // await point anyway, so a synchronous response skips the timer wheel.
        let inner = continuation.scope(self.inner.call(req));
        let result = match self.timeout {
            Some(timeout) => {
                let mut inner = pin!(inner);
                let first = poll_fn(|cx| match inner.as_mut().poll(cx) {
                    Poll::Ready(result) => Poll::Ready(Some(result)),
                    Poll::Pending => Poll::Ready(None),
                })
                .await;
                match first {
                    Some(result) => result,
                    None => match tokio::time::timeout_at(arrival + timeout, &mut inner).await {
                        Ok(result) => result,
                        Err(_) => {
                            tracing::warn!(target: crate::target::HTTP, ?timeout, "request timed out");
                            return self.finish(timed_out(timeout));
                        }
                    },
                }
            }
            None => inner.await,
        };
        if let Some(overrun) = verdict.and_then(|verdict| verdict.get().copied()) {
            return self.finish(refused(overrun, version));
        }
        let resp = match result {
            Ok(resp) => resp,
            Err(err) => rendered(err).await,
        };
        self.finish(resp)
    }
}

impl<E: Endpoint> Endpoint for EdgeEndpoint<E> {
    async fn call(&self, mut req: Request) -> Result<Response> {
        // One instant for the budget and the line: a body that trickles counts
        // against the request it belongs to.
        let arrival = Instant::now();
        // Resolved once so the context, the echoed header and the access line agree
        // on one id; the origin first, because the id's gate reads it.
        let origin = ClientOrigin::of_with(&req, &self.trusted_proxies);
        let correlation = trace_context::resolve(&req, origin);
        let user_agent = trace_context::user_agent(&req);
        // Never gated: it carries `trace_id` onto every event and declares the
        // `actor_id` field the authn guard records into.
        let span = trace_context::request_span(&req, &correlation, origin, user_agent);
        let method = req.method().clone();
        let log = self
            .access_log
            .then(|| AccessLog::open(&req, user_agent, arrival.into_std()));
        // Filled by the router's endpoint, for a request dropped before it answers.
        let matched = MatchedRoute::default();
        req.extensions_mut().insert(matched.clone());

        // Built here, not in `handle`: a streaming body outlives that future.
        let scope = match &self.shared_scope {
            Some(shared) => Arc::clone(shared),
            None => Arc::new(RequestScope::new(self.container.clone())),
        };
        let continuation = RequestContinuation::new(Some(scope), correlation.clone());
        // Held across every await: a request dropped at one (shutdown, client reset)
        // still names its span, records the failure and files its line `cancelled`.
        let unanswered = Unanswered::hold(log, &continuation, &span, &method, &matched);

        // The cap wraps the continuation, not part of it: its readers are extractors,
        // done before a streaming body is written. See `raw_body::with_body_limit`.
        let answer = crate::raw_body::with_body_limit(
            self.body_limit,
            nest_rs_core::panic::contain(
                self.handle(req, &continuation, arrival)
                    .instrument(span.clone()),
            ),
        )
        .await;
        let (resp, outcome) = match answer {
            Ok(resp) => {
                // Before the normalizer, which builds a fresh response without the
                // matched template.
                trace_context::name_route(&span, &method, matched::answered(&resp).as_deref());
                (resp, None)
            }
            Err(payload) => {
                trace_context::name_route(&span, &method, matched.route().as_deref());
                (
                    self.unwound(payload.as_ref(), &continuation, &span),
                    Some(nest_rs_core::operation_log::PANIC),
                )
            }
        };
        let mut resp = match self.normalize {
            true => crate::problem::normalize_error_response(resp).await,
            false => resp,
        };
        // Past the last await: from here the request has an answer to file.
        let log = unanswered.answered();

        span.record("http.response.status_code", resp.status().as_u16());
        trace_context::record_failure(&span, resp.status());
        trace_context::stamp(&correlation, &mut resp);
        // Unconditional: `current_trace_id()` inside a streaming body
        // cannot depend on the access log.
        Ok(response_body::carry(
            continuation,
            span,
            log,
            outcome,
            resp,
            &self.drain,
        ))
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use futures_util::StreamExt;
    use nest_rs_core::current_request_scope;
    use poem::handler;
    use poem::test::TestClient;

    use super::*;
    use crate::endpoint::BoxEndpoint;
    use crate::poem_bridge::{from_poem, to_poem};

    /// The gap bound the edges below hold a body to.
    const GAP: Duration = Duration::from_secs(30);

    fn edge<E>(
        inner: E,
        timeout: Option<Duration>,
        body_limit: Option<usize>,
        headers: Vec<(HeaderName, HeaderValue)>,
    ) -> EdgeEndpoint<E> {
        EdgeEndpoint::new(
            inner,
            Container::builder().build(),
            timeout,
            body_limit.unwrap_or(crate::RawBody::DEFAULT_LIMIT),
            GAP,
            headers,
            false,
            false,
            Arc::default(),
        )
    }

    /// The shape mounted without CORS or compression: the edge runs the normalizer.
    fn fused_edge<E>(
        inner: E,
        body_limit: Option<usize>,
        headers: Vec<(HeaderName, HeaderValue)>,
    ) -> EdgeEndpoint<E> {
        EdgeEndpoint::new(
            inner,
            Container::builder().build(),
            None,
            body_limit.unwrap_or(crate::RawBody::DEFAULT_LIMIT),
            GAP,
            headers,
            true,
            false,
            Arc::default(),
        )
    }

    fn nosniff() -> Vec<(HeaderName, HeaderValue)> {
        vec![(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        )]
    }

    #[handler]
    async fn observe_scope() -> &'static str {
        assert!(
            current_request_scope().is_some(),
            "the edge installs the ambient request scope",
        );
        "ok"
    }

    #[handler]
    async fn echo_len(body: Vec<u8>) -> String {
        body.len().to_string()
    }

    #[handler]
    async fn slow() -> &'static str {
        tokio::time::sleep(Duration::from_millis(200)).await;
        "late"
    }

    #[handler]
    fn sets_header() -> poem::Response {
        poem::Response::builder()
            .header("x-content-type-options", "handler-value")
            .body("ok")
    }

    #[handler]
    async fn panics_after_waiting() -> &'static str {
        tokio::task::yield_now().await;
        panic!("the handler panicked");
    }

    /// Past an await, so the panic unwinds out of a poll rather than a first call.
    #[tokio::test]
    async fn a_handler_panic_answers_500_and_files_its_line_panic() {
        let logs = nest_rs_testing::LogCapture::install();
        let ep = edge(from_poem(panics_after_waiting), None, None, nosniff());
        let resp = TestClient::new(to_poem(ep)).get("/").send().await;
        resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
        resp.assert_content_type("application/problem+json");
        resp.assert_header("x-content-type-options", "nosniff");
        assert!(
            resp.0.headers().contains_key("traceresponse"),
            "the caller can still name the trace: {:?}",
            resp.0.headers(),
        );
        let body = resp
            .0
            .into_body()
            .into_string()
            .await
            .expect("the document is text");
        assert!(!body.contains("the handler panicked"), "{body}");

        let line = logs.expect_one(
            nest_rs_core::operation_log::TARGET,
            crate::unit::REQUEST.name(),
        );
        assert_eq!(
            line.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::PANIC),
            "{line:#?}"
        );
        assert_eq!(line.field("status").as_deref(), Some("500"), "{line:#?}");
        let errors: Vec<_> = logs
            .events()
            .into_iter()
            .filter(|event| event.level == "error")
            .collect();
        assert_eq!(errors.len(), 1, "one event says the panic: {errors:#?}");
        assert_eq!(errors[0].target, crate::target::HTTP);
        assert_eq!(
            errors[0].field(nest_rs_core::panic::FIELD).as_deref(),
            Some("the handler panicked"),
        );
    }

    #[tokio::test]
    async fn with_the_access_log_off_the_span_still_says_the_request_panicked() {
        let logs = nest_rs_testing::LogCapture::install();
        let container = Container::builder()
            .provide(crate::HttpConfig {
                access_log: false,
                ..crate::HttpConfig::default()
            })
            .build();
        let ep = EdgeEndpoint::new(
            from_poem(panics_after_waiting),
            container,
            None,
            crate::RawBody::DEFAULT_LIMIT,
            GAP,
            Vec::new(),
            false,
            false,
            Arc::default(),
        );
        let resp = TestClient::new(to_poem(ep)).get("/").send().await;
        resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
        resp.0
            .into_body()
            .into_string()
            .await
            .expect("the document is read to its end");

        logs.expect_none(
            nest_rs_core::operation_log::TARGET,
            crate::unit::REQUEST.name(),
        );
        let span = logs.expect_span(crate::target::HTTP, crate::unit::REQUEST.name());
        assert_eq!(
            span.field("error.type").as_deref(),
            Some(nest_rs_core::operation_log::PANIC),
            "{span:#?}"
        );
    }

    #[handler]
    fn panics_over_a_decode_failure() -> &'static str {
        let parsed: u64 = serde_json::from_str(r#""sk_live_51HsecretTOKEN""#).expect("a number");
        if parsed == 0 { "zero" } else { "some" }
    }

    #[tokio::test]
    async fn the_logged_payload_is_redacted() {
        let logs = nest_rs_testing::LogCapture::install();
        let ep = edge(
            from_poem(panics_over_a_decode_failure),
            None,
            None,
            Vec::new(),
        );
        let resp = TestClient::new(to_poem(ep)).get("/").send().await;
        resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);

        let said = logs
            .events()
            .into_iter()
            .find(|event| event.level == "error")
            .and_then(|event| event.field(nest_rs_core::panic::FIELD))
            .expect("the panic is said");
        assert!(!said.contains("sk_live"), "{said}");
        assert!(
            said.contains("expected u64"),
            "what failed is still said: {said}"
        );
    }

    #[tokio::test]
    async fn installs_a_request_scope_and_forwards_the_response() {
        let ep = edge(from_poem(observe_scope), None, None, Vec::new());
        let resp = TestClient::new(to_poem(ep)).get("/").send().await;
        resp.assert_status_is_ok();
        resp.assert_text("ok").await;
    }

    #[test]
    fn a_trailing_slash_is_not_part_of_the_path() {
        assert_eq!(canonical_path("/kitchen/"), Some("/kitchen"));
        assert_eq!(canonical_path("/kitchen///"), Some("/kitchen"));
        assert_eq!(
            canonical_path("/v1/kitchen/items/"),
            Some("/v1/kitchen/items")
        );
        assert_eq!(canonical_path("//"), Some("/"));
    }

    #[test]
    fn a_canonical_path_is_left_alone() {
        assert_eq!(canonical_path("/"), None);
        assert_eq!(canonical_path("/kitchen"), None);
        assert_eq!(canonical_path(""), None);
        assert_eq!(canonical_path("/kitchen//items"), None);
    }

    // The rewritten request through a real controller: `tests/integration/edge.rs`.

    #[tokio::test]
    async fn declared_length_over_the_cap_is_rejected_with_headers() {
        // TestClient bypasses the wire, so the Content-Length hyper would derive
        // is set by hand.
        let ep = edge(from_poem(echo_len), None, Some(8), nosniff());
        let resp = TestClient::new(to_poem(ep))
            .post("/")
            .header("content-length", "64")
            .body(vec![0u8; 64])
            .send()
            .await;
        resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
        resp.assert_header("x-content-type-options", "nosniff");
    }

    #[tokio::test]
    async fn declared_length_within_the_cap_passes_the_body_through() {
        let ep = edge(from_poem(echo_len), None, Some(1024), Vec::new());
        let resp = TestClient::new(to_poem(ep))
            .post("/")
            .header("content-length", "64")
            .body(vec![0u8; 64])
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text("64").await;
    }

    #[tokio::test]
    async fn an_undeclared_length_body_is_capped_as_its_reader_takes_it() {
        // No Content-Length: the wrapper counts what arrives.
        let ep = edge(from_poem(echo_len), None, Some(8), Vec::new());
        let resp = TestClient::new(to_poem(ep))
            .post("/")
            .body(vec![0u8; 64])
            .send()
            .await;
        resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn a_bodyless_request_passes_under_the_cap() {
        let ep = edge(from_poem(observe_scope), None, Some(8), Vec::new());
        let resp = TestClient::new(to_poem(ep)).get("/").send().await;
        resp.assert_status_is_ok();
    }

    #[tokio::test]
    async fn overrunning_handler_answers_503_with_a_retry_after_and_headers() {
        let ep = edge(
            from_poem(slow),
            Some(Duration::from_millis(20)),
            None,
            nosniff(),
        );
        let resp = TestClient::new(to_poem(ep)).get("/").send().await;
        resp.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        resp.assert_header(header::RETRY_AFTER, "1");
        resp.assert_header("x-content-type-options", "nosniff");
    }

    #[test]
    fn the_retry_after_a_timeout_states_is_the_budget_that_ran_out() {
        let value = |secs: u64, nanos: u32| {
            timed_out(Duration::new(secs, nanos))
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        assert_eq!(value(30, 0).as_deref(), Some("30"));
        assert_eq!(value(0, 500_000_000).as_deref(), Some("1"), "never zero");
        assert_eq!(
            value(30, 1).as_deref(),
            Some("31"),
            "rounded up — a client that waits less may collide with the attempt",
        );
    }

    #[tokio::test]
    async fn synchronous_response_under_a_timeout_is_forwarded() {
        // The lazy timer's fast path: a first poll that resolves never arms it.
        let ep = edge(
            from_poem(observe_scope),
            Some(Duration::from_secs(30)),
            None,
            Vec::new(),
        );
        let resp = TestClient::new(to_poem(ep)).get("/").send().await;
        resp.assert_status_is_ok();
        resp.assert_text("ok").await;
    }

    #[handler]
    async fn briefly_slow() -> &'static str {
        tokio::time::sleep(Duration::from_millis(5)).await;
        "made it"
    }

    #[tokio::test]
    async fn pending_handler_within_budget_completes_after_arming_the_timer() {
        // The lazy timer's slow path: a first `Pending` arms it.
        let ep = edge(
            from_poem(briefly_slow),
            Some(Duration::from_secs(30)),
            None,
            Vec::new(),
        );
        let resp = TestClient::new(to_poem(ep)).get("/").send().await;
        resp.assert_status_is_ok();
        resp.assert_text("made it").await;
    }

    #[tokio::test]
    async fn a_value_the_handler_set_is_kept_and_a_default_fills_what_it_left_unset() {
        let mut headers = nosniff();
        headers.push((
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ));
        let ep = edge(from_poem(sets_header), None, None, headers);
        let resp = TestClient::new(to_poem(ep)).get("/").send().await;
        resp.assert_status_is_ok();
        resp.assert_header_all("x-content-type-options", ["handler-value"]);
        resp.assert_header("x-frame-options", "DENY");
    }

    #[tokio::test]
    async fn fused_normalize_lifts_a_413_onto_problem_json_with_headers() {
        let ep = fused_edge(from_poem(echo_len), Some(8), nosniff());
        let resp = TestClient::new(to_poem(ep))
            .post("/")
            .header("content-length", "64")
            .body(vec![0u8; 64])
            .send()
            .await;
        resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
        resp.assert_header("x-content-type-options", "nosniff");
        resp.assert_content_type("application/problem+json");
    }

    /// Reads its body to the end and answers `200` whatever the read made of
    /// it: the edge's answer must not depend on how a reader takes a failure.
    #[handler]
    async fn swallows_read_errors(body: poem::Body) -> &'static str {
        let mut frames = body.into_bytes_stream();
        while frames.next().await.is_some() {}
        "read"
    }

    /// One frame, then nothing for ever.
    fn stalling() -> Body {
        Body::from_stream(
            futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"part"))])
                .chain(futures_util::stream::pending()),
        )
    }

    fn post(version: Version, body: Body) -> Request {
        let mut request = Request::builder().method(http::Method::POST).body(body);
        request.head_mut().version = version;
        request
    }

    #[tokio::test]
    async fn a_body_past_the_cap_answers_413_whatever_its_reader_made_of_it() {
        let ep = edge(from_poem(swallows_read_errors), None, Some(8), nosniff());
        let resp = ep
            .call(post(Version::HTTP_11, Body::from(vec![0u8; 64])))
            .await
            .expect("the edge answers");
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_body_answers_408_closing_an_http1_connection() {
        let ep = edge(from_poem(swallows_read_errors), None, Some(1024), nosniff());
        let resp = ep
            .call(post(Version::HTTP_11, stalling()))
            .await
            .expect("the edge answers");
        assert_eq!(resp.status(), StatusCode::REQUEST_TIMEOUT);
        assert_eq!(resp.headers()[header::CONNECTION], "close");
        assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_body_on_http2_answers_408_without_a_connection_header() {
        let ep = edge(
            from_poem(swallows_read_errors),
            None,
            Some(1024),
            Vec::new(),
        );
        let resp = ep
            .call(post(Version::HTTP_2, stalling()))
            .await
            .expect("the edge answers");
        assert_eq!(resp.status(), StatusCode::REQUEST_TIMEOUT);
        assert!(resp.headers().get(header::CONNECTION).is_none());
    }

    fn failing() -> impl Endpoint {
        crate::endpoint_fn(|_| async { Err(HttpError::from_status(StatusCode::BAD_REQUEST)) })
    }

    #[tokio::test]
    async fn an_inner_error_is_rendered_inside_the_header_stamp() {
        for ep in [
            BoxEndpoint::new(fused_edge(failing(), None, nosniff())),
            BoxEndpoint::new(edge(failing(), None, None, nosniff())),
        ] {
            let resp = TestClient::new(to_poem(ep)).get("/").send().await;
            resp.assert_status(StatusCode::BAD_REQUEST);
            resp.assert_content_type("application/problem+json");
            resp.assert_header("x-content-type-options", "nosniff");
        }
    }
}
