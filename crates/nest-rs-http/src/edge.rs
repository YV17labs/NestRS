//! The fused transport-edge endpoint: trailing-slash trim, request scope, body
//! cap, request timeout and default response headers, in that order, in one layer.
//!
//! A `413` or `503` is produced inside the header stamp and carries the security
//! headers; an `Err` escaping the inner tree carries none, and the problem
//! normalizer renders it — run by the edge itself when it is outermost (`normalize`).

use std::future::{Future, poll_fn};
use std::net::IpAddr;
use std::pin::pin;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use std::time::Duration;

use futures_util::StreamExt;
use nest_rs_core::{Container, RequestContinuation, RequestScope};
use poem::error::ReadBodyError;
use poem::http::uri::{PathAndQuery, Uri};
use poem::http::{HeaderName, HeaderValue, StatusCode};
use poem::web::headers::{ContentLength, HeaderMapExt};
use poem::{Body, Endpoint, IntoResponse, PathPattern, Request, Response, Result};

use tracing::Instrument;

use crate::access_log::{AccessLog, Unanswered};
use crate::client_ip::ClientOrigin;
use crate::drain::Drain;
use crate::location::CallerUri;
use crate::matched::MatchedRoute;
use crate::{response_body, trace_context};

/// The route template poem's router matched, off the response or the error;
/// `None` when nothing matched.
fn matched_route(result: &Result<Response>) -> Option<&str> {
    let pattern = match result {
        Ok(resp) => resp.data::<PathPattern>(),
        Err(err) => err.data::<PathPattern>(),
    };
    pattern.map(|PathPattern(pattern)| &**pattern)
}

fn bare(status: StatusCode) -> Response {
    Response::builder().status(status).finish()
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
        .header(poem::http::header::RETRY_AFTER, seconds)
        .finish()
}

/// What a body past the cap fails the stream with; the caller gets the edge's
/// `413` instead ([`CapExceeded`]).
fn body_cap_exceeded() -> String {
    format!(
        "request body exceeded the configured cap ({})",
        nest_rs_config::var_name("http", "MAX_BODY_BYTES"),
    )
}

/// Set by [`capped`] past the cap and read by the edge once the inner tree has
/// returned, so the cap answers `413` whatever the reading extractor made of it.
type CapExceeded = Arc<AtomicBool>;

/// The cap, enforced on the bytes that actually arrive: poem's `Compression`
/// decompresses the body and leaves `Content-Length` describing the compressed bytes.
fn capped(body: Body, limit: usize, exceeded: CapExceeded) -> Body {
    let mut seen: usize = 0;
    Body::from_bytes_stream(body.into_bytes_stream().map(move |chunk| {
        let chunk = chunk?;
        seen = seen.saturating_add(chunk.len());
        if seen > limit {
            exceeded.store(true, Ordering::Relaxed);
            return Err(std::io::Error::other(body_cap_exceeded()));
        }
        Ok(chunk)
    }))
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
    /// `None`: body readers fall back to their own default
    /// ([`current_body_limit`](crate::current_body_limit)).
    body_limit: Option<usize>,
    /// Stamped with replace semantics: the framework value wins over a handler's.
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
        body_limit: Option<usize>,
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
            headers,
            normalize,
            counts_body,
            access_log,
            trusted_proxies,
            drain,
        }
    }

    fn finish(&self, mut resp: Response) -> Response {
        for (name, value) in &self.headers {
            resp.headers_mut().insert(name.clone(), value.clone());
        }
        resp
    }
}

impl<E> EdgeEndpoint<E>
where
    E: Endpoint,
    E::Output: IntoResponse,
{
    /// The inner tree, under the context `call` built and installs again
    /// around the response body.
    async fn handle(
        &self,
        mut req: Request,
        continuation: &RequestContinuation,
    ) -> Result<Response> {
        // The router matches exactly: `/kitchen/` would 404 before any guard runs.
        trim_trailing_slash(&mut req);

        // The last point that sees the URI whole: the router strips a global prefix
        // off `uri()`, and `original_uri()` is set on the hyper path only.
        let caller_uri = req.uri().clone();
        req.extensions_mut().insert(CallerUri(caller_uri));

        // Body cap. A declared length is a claim, counted only where an
        // outer layer can falsify it (`counts_body`); with none declared, buffer up to the cap.
        let mut exceeded: Option<CapExceeded> = None;
        if let Some(limit) = self.body_limit {
            let declared = req
                .headers()
                .typed_get::<ContentLength>()
                .map(|ContentLength(declared)| declared as usize);
            if declared.is_some_and(|declared| declared > limit) {
                return Ok(self.finish(bare(StatusCode::PAYLOAD_TOO_LARGE)));
            }
            let body = req.take_body();
            if body.is_empty() {
                req.set_body(body);
            } else if declared.is_some() {
                if self.counts_body {
                    let flag = CapExceeded::default();
                    req.set_body(capped(body, limit, Arc::clone(&flag)));
                    exceeded = Some(flag);
                } else {
                    req.set_body(body);
                }
            } else {
                match body.into_bytes_limit(limit).await {
                    Ok(bytes) => req.set_body(bytes),
                    Err(ReadBodyError::PayloadTooLarge) => {
                        return Ok(self.finish(bare(StatusCode::PAYLOAD_TOO_LARGE)));
                    }
                    Err(err) => return Err(err.into()),
                }
            }
        }

        // The timer arms on the first `Pending`: a timeout only fires at an await
        // point anyway, so a synchronous response skips the timer wheel.
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
                    None => match tokio::time::timeout(timeout, &mut inner).await {
                        Ok(result) => result,
                        Err(_) => {
                            tracing::warn!(target: crate::target::HTTP, ?timeout, "request timed out");
                            return Ok(self.finish(timed_out(timeout)));
                        }
                    },
                }
            }
            None => inner.await,
        };
        if exceeded.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Ok(self.finish(bare(StatusCode::PAYLOAD_TOO_LARGE)));
        }
        Ok(self.finish(result?.into_response()))
    }
}

impl<E> Endpoint for EdgeEndpoint<E>
where
    E: Endpoint,
    E::Output: IntoResponse,
{
    type Output = Response;

    async fn call(&self, mut req: Request) -> Result<Response> {
        // Resolved once so the context, the echoed header and the access line agree
        // on one id; the origin first, because the id's gate reads it.
        let origin = ClientOrigin::of_with(&req, &self.trusted_proxies);
        let correlation = trace_context::resolve(&req, origin);
        let user_agent = trace_context::user_agent(&req);
        // Never gated: it carries `trace_id` onto every event and declares the
        // `actor_id` field the authn guard records into.
        let span = trace_context::request_span(&req, &correlation, origin, user_agent);
        let method = req.method().clone();
        let log = self.access_log.then(|| AccessLog::open(&req, user_agent));
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
        let result = crate::raw_body::with_body_limit(
            self.body_limit,
            self.handle(req, &continuation).instrument(span.clone()),
        )
        .await;
        // Before any `Err` is rendered: rendering builds a fresh response without
        // the matched template poem attached.
        trace_context::name_route(&span, &method, matched_route(&result));
        let result = if self.normalize {
            // An `Err` renders without the header stamp.
            Ok(match result {
                Ok(resp) => crate::problem::normalize_error_response(resp).await,
                Err(err) => {
                    crate::problem::normalize_error_response(crate::problem::render_error(err))
                        .await
                }
            })
        } else {
            result
        };
        // Past the last await: from here the request has an answer to file.
        let log = unanswered.answered();

        match result {
            Ok(mut resp) => {
                span.record("http.response.status_code", resp.status().as_u16());
                trace_context::record_failure(&span, resp.status());
                trace_context::stamp(&correlation, &mut resp);
                // Unconditional: `current_trace_id()` inside a streaming body
                // cannot depend on the access log.
                Ok(response_body::carry(
                    continuation,
                    span,
                    log,
                    resp,
                    &self.drain,
                ))
            }
            // Only with CORS or compression, whose outer wrap renders the error.
            Err(err) => {
                span.record("http.response.status_code", err.status().as_u16());
                trace_context::record_failure(&span, err.status());
                if let Some(log) = log {
                    log.abandoned(&span, err.status().as_u16());
                }
                Err(err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::current_request_scope;
    use poem::EndpointExt;
    use poem::handler;
    use poem::test::TestClient;

    use super::*;

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
            body_limit,
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
            body_limit,
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
    fn sets_header() -> Response {
        Response::builder()
            .header("x-content-type-options", "handler-value")
            .body("ok")
    }

    #[handler]
    async fn panics_after_waiting() -> &'static str {
        tokio::task::yield_now().await;
        panic!("the handler panicked");
    }

    /// Past an await, so the panic unwinds through the edge's state rather than a drop.
    #[tokio::test]
    async fn a_handler_that_panics_files_its_line_panic() {
        use futures_util::FutureExt;

        let logs = nest_rs_testing::LogCapture::install();
        let ep = edge(panics_after_waiting, None, None, Vec::new());
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let unwound = std::panic::AssertUnwindSafe(TestClient::new(ep).get("/").send())
            .catch_unwind()
            .await;
        std::panic::set_hook(previous);
        assert!(unwound.is_err(), "the panic reached the caller");

        let line = logs.expect_one(
            nest_rs_core::operation_log::TARGET,
            crate::unit::REQUEST.name(),
        );
        assert_eq!(
            line.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::PANIC),
            "{line:#?}"
        );
        assert_eq!(line.field("status"), None);
    }

    #[tokio::test]
    async fn installs_a_request_scope_and_forwards_the_response() {
        let ep = edge(observe_scope, None, None, Vec::new());
        let resp = TestClient::new(ep).get("/").send().await;
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
        let ep = edge(echo_len, None, Some(8), nosniff());
        let resp = TestClient::new(ep)
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
        let ep = edge(echo_len, None, Some(1024), Vec::new());
        let resp = TestClient::new(ep)
            .post("/")
            .header("content-length", "64")
            .body(vec![0u8; 64])
            .send()
            .await;
        resp.assert_status_is_ok();
        resp.assert_text("64").await;
    }

    #[tokio::test]
    async fn undeclared_length_body_is_buffered_and_capped() {
        // No Content-Length forces the buffered path.
        let ep = edge(echo_len, None, Some(8), Vec::new());
        let resp = TestClient::new(ep)
            .post("/")
            .body(vec![0u8; 64])
            .send()
            .await;
        resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn a_bodyless_request_passes_under_the_cap() {
        let ep = edge(observe_scope, None, Some(8), Vec::new());
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status_is_ok();
    }

    #[tokio::test]
    async fn overrunning_handler_answers_503_with_a_retry_after_and_headers() {
        let ep = edge(slow, Some(Duration::from_millis(20)), None, nosniff());
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        resp.assert_header(poem::http::header::RETRY_AFTER, "1");
        resp.assert_header("x-content-type-options", "nosniff");
    }

    #[test]
    fn the_retry_after_a_timeout_states_is_the_budget_that_ran_out() {
        let value = |secs: u64, nanos: u32| {
            timed_out(Duration::new(secs, nanos))
                .headers()
                .get(poem::http::header::RETRY_AFTER)
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
            observe_scope,
            Some(Duration::from_secs(30)),
            None,
            Vec::new(),
        );
        let resp = TestClient::new(ep).get("/").send().await;
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
            briefly_slow,
            Some(Duration::from_secs(30)),
            None,
            Vec::new(),
        );
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status_is_ok();
        resp.assert_text("made it").await;
    }

    #[tokio::test]
    async fn security_header_replaces_a_handler_set_value() {
        let ep = edge(sets_header, None, None, nosniff());
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status_is_ok();
        resp.assert_header("x-content-type-options", "nosniff");
    }

    #[tokio::test]
    async fn fused_normalize_lifts_a_413_onto_problem_json_with_headers() {
        let ep = fused_edge(echo_len, Some(8), nosniff());
        let resp = TestClient::new(ep)
            .post("/")
            .header("content-length", "64")
            .body(vec![0u8; 64])
            .send()
            .await;
        resp.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
        resp.assert_header("x-content-type-options", "nosniff");
        resp.assert_content_type("application/problem+json");
    }

    #[tokio::test]
    async fn fused_normalize_renders_an_inner_error_without_the_header_stamp() {
        let failing = poem::endpoint::make(|_| async {
            Err::<Response, poem::Error>(poem::Error::from_status(StatusCode::BAD_REQUEST))
        });
        let ep = fused_edge(failing, None, nosniff());
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status(StatusCode::BAD_REQUEST);
        resp.assert_content_type("application/problem+json");
        assert!(
            resp.0.headers().get("x-content-type-options").is_none(),
            "errors bypass the header stamp, matching the standalone normalizer",
        );
    }

    #[tokio::test]
    async fn an_inner_error_propagates_without_headers() {
        let failing = poem::endpoint::make(|_| async {
            Err::<Response, poem::Error>(poem::Error::from_status(StatusCode::BAD_REQUEST))
        });
        let ep = edge(failing, None, None, nosniff()).map_to_response();
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status(StatusCode::BAD_REQUEST);
        assert!(
            resp.0.headers().get("x-content-type-options").is_none(),
            "errors bypass the header stamp, matching the previous SetHeader behavior",
        );
    }
}
