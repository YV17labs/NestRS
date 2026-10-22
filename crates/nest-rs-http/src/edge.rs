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
use http::{HeaderName, HeaderValue, Version, header};
use http_body_util::combinators::BoxBody;
use nest_rs_core::{Container, RequestContinuation, RequestScope};
use tokio::time::Instant;

use tracing::Instrument;

use crate::access_log::{AccessLog, Unanswered};
use crate::body::Body;
use crate::capped::{Capped, Overrun, Verdict};
use crate::client_ip::ClientOrigin;
use crate::deadline::RequestDeadline;
use crate::drain::Drain;
use crate::endpoint::Endpoint;
use crate::error::{ResponseError, Result};
use crate::location::CallerUri;
use crate::matched::MatchedRoute;
use crate::problem::ProblemDetails;
use crate::request::Request;
use crate::response::{IntoResponse, Response};
use crate::trace_context::KnownMethods;
use crate::{response_body, trace_context};

/// What a request whose unit of work did not settle within its `net` past the
/// deadline is answered: a `500` without `Retry-After`, since what it wrote may
/// have landed, said once at `error`.
fn unsettled(net: Duration) -> Response {
    tracing::error!(
        target: crate::target::HTTP,
        net_ms = crate::server::millis(net),
        "request's unit of work did not settle within its net; its writes may have landed",
    );
    ProblemDetails::internal().into_response()
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
    /// The methods the line and the span name as sent.
    known_methods: KnownMethods,
    drain: Arc<Drain>,
}

impl<E> EdgeEndpoint<E> {
    #[expect(
        clippy::too_many_arguments,
        reason = "one value per decision the transport hands its edge, each a field read once \
                  per request; one call site, `HttpTransport::edge`"
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
        known_methods: KnownMethods,
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
            known_methods,
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

        // A unit of work under the request takes the cut over through it.
        let deadline = self.timeout.map(|timeout| {
            let deadline = RequestDeadline::new(arrival + timeout, timeout);
            req.extensions_mut().insert(deadline.clone());
            deadline
        });
        // The deadline arms on the first `Pending`: a timeout only fires at an
        // await point anyway, so a synchronous response skips the timer wheel.
        let inner = continuation.scope(self.inner.call(req));
        let result = match deadline {
            Some(deadline) => {
                let mut inner = pin!(inner);
                let first = poll_fn(|cx| match inner.as_mut().poll(cx) {
                    Poll::Ready(result) => Poll::Ready(Some(result)),
                    Poll::Pending => Poll::Ready(None),
                })
                .await;
                match first {
                    Some(result) => result,
                    None => match tokio::time::timeout_at(deadline.at(), &mut inner).await {
                        Ok(result) => result,
                        Err(_) => match deadline.unit_net() {
                            None => return self.finish(deadline.timed_out()),
                            // The unit cut its body there itself: past its net,
                            // its commit never answered.
                            Some(net) => {
                                match tokio::time::timeout_at(deadline.at() + net, &mut inner).await
                                {
                                    Ok(result) => result,
                                    Err(_) => return self.finish(unsettled(net)),
                                }
                            }
                        },
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
            Err(err) => crate::problem::normalize_error_response(err.answer()).await,
        };
        self.finish(resp)
    }
}

impl<E: Endpoint> Endpoint for EdgeEndpoint<E> {
    async fn call(&self, mut req: Request) -> Result<Response> {
        // One instant for the budget and the line: a body that trickles counts
        // against the request it belongs to, and a request that waited before
        // the edge ran it against its own.
        let arrival = req.take_arrival().unwrap_or_else(Instant::now);
        // Resolved once so the context, the echoed header and the access line agree
        // on one id; the origin first, because the id's gate reads it.
        let origin = ClientOrigin::of_with(&req, &self.trusted_proxies);
        let correlation = trace_context::resolve(&req, origin);
        let method = self.known_methods.name(req.method());
        // Never gated: it carries `trace_id` onto every event and declares the
        // `actor_id` field the authn guard records into.
        let span = trace_context::request_span(&req, &correlation, origin, &method);
        // Filled by the table's entry the router matched.
        let matched = MatchedRoute::default();
        let log = self
            .access_log
            .then(|| AccessLog::open(method.clone(), &matched, arrival.into_std()));
        req.extensions_mut().insert(matched.clone());
        // The peer a unit's context names, resolved once.
        req.extensions_mut().insert(origin);

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
        trace_context::name_route(&span, &method, matched.route().as_deref());
        let (resp, outcome) = match answer {
            Ok(resp) => (resp, None),
            Err(payload) => (
                self.unwound(payload.as_ref(), &continuation, &span),
                Some(nest_rs_core::operation_log::PANIC),
            ),
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

    use http::StatusCode;

    use super::*;
    use crate::endpoint::{BoxEndpoint, endpoint_fn};
    use crate::error::HttpError;
    use crate::testing::{answer, get, internal_document, text};

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
            Container::builder().build().expect("the container builds"),
            timeout,
            body_limit.unwrap_or(crate::RawBody::DEFAULT_LIMIT),
            GAP,
            headers,
            false,
            false,
            KnownMethods::default(),
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
            Container::builder().build().expect("the container builds"),
            None,
            body_limit.unwrap_or(crate::RawBody::DEFAULT_LIMIT),
            GAP,
            headers,
            true,
            false,
            KnownMethods::default(),
            Arc::default(),
        )
    }

    fn opaque_document(body: &str) -> serde_json::Value {
        serde_json::from_str(body).expect("problem json")
    }

    fn nosniff() -> Vec<(HeaderName, HeaderValue)> {
        vec![(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        )]
    }

    fn observe_scope() -> impl Endpoint {
        endpoint_fn(|_| async {
            assert!(
                current_request_scope().is_some(),
                "the edge installs the ambient request scope",
            );
            Ok(Response::builder().body("ok"))
        })
    }

    /// Reads the whole body and answers its length; a read the edge refused
    /// answers as that refusal.
    fn echo_len() -> impl Endpoint {
        endpoint_fn(|req: Request| async move {
            let body = req.into_body().into_bytes().await?;
            Ok(Response::builder().body(body.len().to_string()))
        })
    }

    fn slow() -> impl Endpoint {
        endpoint_fn(|_| async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(Response::builder().body("late"))
        })
    }

    fn sets_header() -> impl Endpoint {
        endpoint_fn(|_| async {
            Ok(Response::builder()
                .header(
                    header::X_CONTENT_TYPE_OPTIONS,
                    HeaderValue::from_static("handler-value"),
                )
                .body("ok"))
        })
    }

    fn panics_after_waiting() -> impl Endpoint {
        endpoint_fn(|_| async {
            tokio::task::yield_now().await;
            panic!("the handler panicked");
        })
    }

    /// Past an await, so the panic unwinds out of a poll rather than a first call.
    #[tokio::test]
    async fn a_handler_panic_answers_500_and_files_its_line_panic() {
        let logs = nest_rs_testing::LogCapture::install();
        let ep = edge(panics_after_waiting(), None, None, nosniff());
        let resp = answer(&ep, get()).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
        assert!(
            resp.headers().contains_key("traceresponse"),
            "the caller can still name the trace: {:?}",
            resp.headers(),
        );
        let body = text(resp).await;
        assert!(!body.contains("the handler panicked"), "{body}");
        assert_eq!(opaque_document(&body), internal_document(), "{body}");

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
        let errors = logs.events_at(tracing::Level::ERROR);
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
            .build()
            .expect("the container builds");
        let ep = EdgeEndpoint::new(
            panics_after_waiting(),
            container,
            None,
            crate::RawBody::DEFAULT_LIMIT,
            GAP,
            Vec::new(),
            false,
            false,
            KnownMethods::default(),
            Arc::default(),
        );
        let resp = answer(&ep, get()).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        text(resp).await;

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

    fn panics_over_a_decode_failure() -> impl Endpoint {
        endpoint_fn(|_| async {
            let parsed: u64 =
                serde_json::from_str(r#""sk_live_51HsecretTOKEN""#).expect("a number");
            Ok(Response::builder().body(if parsed == 0 { "zero" } else { "some" }))
        })
    }

    #[tokio::test]
    async fn the_logged_payload_is_redacted() {
        let logs = nest_rs_testing::LogCapture::install();
        let ep = edge(panics_over_a_decode_failure(), None, None, Vec::new());
        let resp = answer(&ep, get()).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let said = logs
            .events_at(tracing::Level::ERROR)
            .first()
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
        let ep = edge(observe_scope(), None, None, Vec::new());
        let resp = answer(&ep, get()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(text(resp).await, "ok");
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
        let ep = edge(echo_len(), None, Some(8), nosniff());
        let resp = answer(&ep, declaring_64_bytes()).await;
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
    }

    #[tokio::test]
    async fn declared_length_within_the_cap_passes_the_body_through() {
        let ep = edge(echo_len(), None, Some(1024), Vec::new());
        let resp = answer(&ep, declaring_64_bytes()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(text(resp).await, "64");
    }

    #[tokio::test]
    async fn an_undeclared_length_body_is_capped_as_its_reader_takes_it() {
        // No Content-Length: the wrapper counts what arrives.
        let ep = edge(echo_len(), None, Some(8), Vec::new());
        let resp = answer(&ep, post(Version::HTTP_11, Body::from(vec![0u8; 64]))).await;
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn a_bodyless_request_passes_under_the_cap() {
        let ep = edge(observe_scope(), None, Some(8), Vec::new());
        let resp = answer(&ep, get()).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn overrunning_handler_answers_503_with_a_retry_after_and_headers() {
        let ep = edge(slow(), Some(Duration::from_millis(20)), None, nosniff());
        let resp = answer(&ep, get()).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(resp.headers()[header::RETRY_AFTER], "1");
        assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
    }

    #[tokio::test]
    async fn synchronous_response_under_a_timeout_is_forwarded() {
        // The lazy timer's fast path: a first poll that resolves never arms it.
        let ep = edge(
            observe_scope(),
            Some(Duration::from_secs(30)),
            None,
            Vec::new(),
        );
        let resp = answer(&ep, get()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(text(resp).await, "ok");
    }

    fn briefly_slow() -> impl Endpoint {
        endpoint_fn(|_| async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            Ok(Response::builder().body("made it"))
        })
    }

    #[tokio::test]
    async fn pending_handler_within_budget_completes_after_arming_the_timer() {
        // The lazy timer's slow path: a first `Pending` arms it.
        let ep = edge(
            briefly_slow(),
            Some(Duration::from_secs(30)),
            None,
            Vec::new(),
        );
        let resp = answer(&ep, get()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(text(resp).await, "made it");
    }

    /// The budget runs from the instant the accept loop handed the request
    /// over, so a request that waited before the edge ran it has less left.
    #[tokio::test(start_paused = true)]
    async fn the_budget_counts_from_the_requests_arrival() {
        let arrival = Instant::now();
        tokio::time::advance(Duration::from_secs(4)).await;
        let answers_in_two = crate::endpoint_fn(|_| async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Ok(Response::builder().body("late"))
        });
        let ep = edge(
            answers_in_two,
            Some(Duration::from_secs(5)),
            None,
            Vec::new(),
        );
        let (head, ()) = http::Request::new(()).into_parts();
        let facts = crate::request::RequestFacts {
            peer: None,
            local: None,
            scheme: http::uri::Scheme::HTTP,
            original_uri: head.uri.clone(),
            route: None,
            arrival: Some(arrival),
        };
        let resp = ep
            .call(Request::from_raw(head, Body::empty(), facts))
            .await
            .expect("the edge answers");
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// A request with no peer, arrived now, as the accept loop hands one over.
    fn arrived() -> Request {
        let (head, ()) = http::Request::new(()).into_parts();
        let facts = crate::request::RequestFacts {
            peer: None,
            local: None,
            scheme: http::uri::Scheme::HTTP,
            original_uri: head.uri.clone(),
            route: None,
            arrival: Some(Instant::now()),
        };
        Request::from_raw(head, Body::empty(), facts)
    }

    /// A unit of work that took the request's deadline over, then answers
    /// after `answers`, or never.
    fn unit_answering(answers: Option<Duration>) -> impl Endpoint {
        crate::endpoint_fn(move |req: Request| async move {
            let deadline = req
                .extensions()
                .get::<crate::deadline::RequestDeadline>()
                .cloned()
                .expect("the edge hands its deadline down");
            deadline.hand_to_unit(Duration::from_secs(2));
            match answers {
                Some(after) => tokio::time::sleep(after).await,
                None => std::future::pending().await,
            }
            Ok(Response::builder()
                .status(StatusCode::CREATED)
                .body("committed"))
        })
    }

    #[tokio::test(start_paused = true)]
    async fn a_unit_that_took_the_deadline_over_is_waited_for_within_its_net() {
        let ep = edge(
            unit_answering(Some(Duration::from_secs(6))),
            Some(Duration::from_secs(5)),
            None,
            Vec::new(),
        );
        let resp = ep.call(arrived()).await.expect("the edge answers");
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "a commit past the deadline is the request's answer"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_unit_that_never_settles_is_answered_500_without_retry_after_at_its_net() {
        let logs = nest_rs_testing::LogCapture::install();
        let started = Instant::now();
        let ep = edge(
            unit_answering(None),
            Some(Duration::from_secs(5)),
            None,
            Vec::new(),
        );
        let resp = ep.call(arrived()).await.expect("the edge answers");
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            resp.headers().get(header::RETRY_AFTER).is_none(),
            "what may have landed is never invited again"
        );
        assert_eq!(started.elapsed(), Duration::from_secs(7));
        assert_eq!(opaque_document(&text(resp).await), internal_document());
        let line = logs.expect_one(
            crate::target::HTTP,
            "request's unit of work did not settle within its net; its writes may have landed",
        );
        assert_eq!(line.level, "error");
    }

    #[tokio::test]
    async fn a_value_the_handler_set_is_kept_and_a_default_fills_what_it_left_unset() {
        let mut headers = nosniff();
        headers.push((
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ));
        let ep = edge(sets_header(), None, None, headers);
        let resp = answer(&ep, get()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get_all("x-content-type-options")
                .iter()
                .collect::<Vec<_>>(),
            ["handler-value"],
            "the handler's value is kept, never doubled",
        );
        assert_eq!(resp.headers()["x-frame-options"], "DENY");
    }

    #[tokio::test]
    async fn fused_normalize_lifts_a_413_onto_problem_json_with_headers() {
        let ep = fused_edge(echo_len(), Some(8), nosniff());
        let resp = answer(&ep, declaring_64_bytes()).await;
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
    }

    /// Reads its body to the end and answers `200` whatever the read made of
    /// it: the edge's answer must not depend on how a reader takes a failure.
    fn swallows_read_errors() -> impl Endpoint {
        endpoint_fn(|req: Request| async move {
            let mut frames = std::pin::pin!(req.into_body().into_stream());
            while frames.next().await.is_some() {}
            Ok(Response::builder().body("read"))
        })
    }

    /// One frame, then nothing for ever.
    fn stalling() -> Body {
        Body::from_stream(
            futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"part"))])
                .chain(futures_util::stream::pending()),
        )
    }

    /// A `POST` whose `Content-Length` says 64 bytes, as hyper derives it on
    /// the wire, and carries them.
    fn declaring_64_bytes() -> Request {
        Request::builder()
            .method(http::Method::POST)
            .header(header::CONTENT_LENGTH, HeaderValue::from_static("64"))
            .body(vec![0u8; 64])
    }

    fn post(version: Version, body: Body) -> Request {
        let mut request = Request::builder().method(http::Method::POST).body(body);
        request.head_mut().version = version;
        request
    }

    #[tokio::test]
    async fn a_body_past_the_cap_answers_413_whatever_its_reader_made_of_it() {
        let ep = edge(swallows_read_errors(), None, Some(8), nosniff());
        let resp = ep
            .call(post(Version::HTTP_11, Body::from(vec![0u8; 64])))
            .await
            .expect("the edge answers");
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_body_answers_408_closing_an_http1_connection() {
        let ep = edge(swallows_read_errors(), None, Some(1024), nosniff());
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
        let ep = edge(swallows_read_errors(), None, Some(1024), Vec::new());
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
            let resp = answer(&ep, get()).await;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
            assert_eq!(
                resp.headers()[header::CONTENT_TYPE],
                "application/problem+json"
            );
            assert_eq!(resp.headers()["x-content-type-options"], "nosniff");
        }
    }
}
