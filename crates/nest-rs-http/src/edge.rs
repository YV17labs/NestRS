//! The fused transport-edge endpoint — one layer carrying the per-request
//! concerns every route shares: path normalization, request-scope install,
//! body-size cap, request timeout, and the default response headers
//! (security + `Server`).
//!
//! Before the fusion each concern was its own boxed wrap (an `.around()`
//! closure or a poem middleware), so every request paid one virtual dispatch
//! and one future state machine per concern — even when the concern amounted
//! to a single branch. Fusing them keeps the documented semantics and
//! relative order exactly (scope, then body cap, then timeout around the
//! inner tree, headers stamped on the way out) at the cost of a single
//! dispatch.
//!
//! Order and error-path behavior mirror the previous layered composition:
//!
//! - The trailing slash is trimmed first, so routing, guards, interceptors and
//!   the route table all see one spelling of a path. It lives here rather than
//!   in a `NormalizePath` middleware for the reason the rest of the layer does:
//!   the fast path is a single `ends_with('/')` test, with no extra boxed
//!   endpoint and no allocation when the path is already canonical.
//! - The request scope is installed before anything inward can resolve
//!   `#[injectable(scope = request)]` providers via [`Scoped`](crate::Scoped).
//! - A `413` (body cap) and a `504` (timeout) are produced *inside* the
//!   header stamp, so they carry the security headers — as they did when the
//!   `SetHeader` middleware wrapped those layers.
//! - An `Err` escaping the inner tree propagates *without* headers, exactly
//!   like `SetHeader` (which forwards errors untouched); the transport-edge
//!   problem normalizer turns it into `problem+json`.
//!
//! The problem normalizer itself follows the same fusion logic: when neither
//! CORS nor compression is configured the edge is the outermost layer, so it
//! runs [`normalize_error_response`](crate::problem::normalize_error_response)
//! as its own tail (`normalize = true`) instead of mounting the `.around`
//! wrap that used to carry it — one boxed layer less on every request. With
//! CORS / compression the wrap stays outermost, unchanged.

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

use crate::access_log::AccessLog;
use crate::client_ip::{ClientIp, ClientOrigin};
use crate::location::CallerUri;
use crate::{response_body, trace_context};

/// The route template poem's router matched, off whichever shape came back.
///
/// It is attached to the response *and* to the error, so a 404 that never
/// reached a controller and a handler's `Err` are both answerable — and a
/// request that matched nothing yields `None`, which is the honest answer rather
/// than the path it was aiming at.
fn matched_route(result: &Result<Response>) -> Option<&str> {
    let pattern = match result {
        Ok(resp) => resp.data::<PathPattern>(),
        Err(err) => err.data::<PathPattern>(),
    };
    pattern.map(|PathPattern(pattern)| &**pattern)
}

/// A bare status-only response — the edge's own rejections (`413`, `504`).
fn bare(status: StatusCode) -> Response {
    Response::builder().status(status).finish()
}

/// What a body that outran the cap fails the *stream* with.
///
/// Whatever extractor was reading sees this; the caller does not, because the
/// edge answers the cap itself (see [`CapExceeded`]). Built rather than spelled:
/// `NESTRS` is the deployment's default prefix, not a fixture.
fn body_cap_exceeded() -> String {
    format!(
        "request body exceeded the configured cap ({})",
        nest_rs_config::var_name("http", "MAX_BODY_BYTES"),
    )
}

/// Set by [`capped`] when the count runs past the cap, read by the edge once the
/// inner tree has returned.
///
/// The cap is one declaration, so it answers with one status. Without this the
/// stream error surfaced as whatever the reading extractor made of it — a `500`
/// from one, a framing error from another — while the *declared-length* branch
/// answered a clean `413`, which made the status a function of the framing the
/// caller happened to choose.
type CapExceeded = Arc<AtomicBool>;

/// The cap, enforced on the bytes that actually arrive.
///
/// A `Content-Length` within the cap is a *claim*, and one an outer layer can
/// make false: poem's `Compression` middleware replaces the request body with a
/// decompressed reader and leaves the header describing the compressed bytes.
/// Anything downstream that streams the body rather than buffering it — an
/// upload going to object storage — then has no bound at all.
///
/// So the count is what bounds it: the read stops, nothing downstream completes
/// on a body it was never allowed to receive, and `exceeded` tells the edge to
/// answer `413` rather than let the failure wear the reader's own error.
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

/// The path a trailing slash should have been written as, or `None` when the
/// path is already canonical (the overwhelming majority — no allocation, no
/// URI rebuild on the hot path).
///
/// `/kitchen/` → `/kitchen`; `//` → `/`; `/` and `/kitchen` are untouched. Only
/// the trailing run is touched: an interior `//` is left alone, because an
/// empty segment mid-path is a different path, not a slip of the pen.
fn canonical_path(path: &str) -> Option<&str> {
    if !path.ends_with('/') || path == "/" {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    // `//`, `///`, … all name the root.
    Some(if trimmed.is_empty() { "/" } else { trimmed })
}

/// Rewrite the request URI onto [`canonical_path`], query preserved.
///
/// Runs before the edge captures [`CallerUri`], so anything echoing the request
/// back (the `Location` on a `#[crud]` create) names the canonical path rather
/// than the caller's trailing slash — one spelling per resource, whichever one
/// was typed.
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
    // Both halves came out of a URI that already parsed, so a failure here is
    // unreachable — and if it ever were reachable, leaving the path as sent is
    // the safe outcome: a 404, never a request routed somewhere else.
    let Ok(path_and_query) = PathAndQuery::from_str(&rebuilt) else {
        return;
    };
    // Cloned rather than `mem::take`n out of the request: `Uri::from_parts` is
    // fallible, and a take that failed would leave the request holding
    // `Uri::default()` — routing every such request to `/` instead of leaving
    // it where the caller aimed it. The clone costs one refcount pair, and only
    // on the branch that already decided to rewrite.
    let mut parts = req.uri().clone().into_parts();
    parts.path_and_query = Some(path_and_query);
    if let Ok(uri) = Uri::from_parts(parts) {
        *req.uri_mut() = uri;
    }
}

/// The single transport-edge layer assembled by
/// [`HttpTransport::configure`](crate::HttpTransport). Wraps the fully
/// composed route tree (per-route layers + `HttpEndpointWrap` globals);
/// CORS / compression (when configured) wrap *outside* it, with the problem
/// normalizer outermost — or fused into this edge when neither is present.
pub(crate) struct EdgeEndpoint<E> {
    inner: E,
    container: Container,
    /// Present when the app registers **no** request-scoped or transient
    /// provider. Such a scope can never cache anything — every resolution
    /// falls through to the singleton container — so one shared instance is
    /// indistinguishable from a fresh one, and costs an `Arc` clone instead
    /// of an allocation on every request.
    shared_scope: Option<Arc<RequestScope>>,
    /// Wall-clock budget for the inner tree; `None` ⇒ no timeout enforced.
    timeout: Option<Duration>,
    /// Raw-body byte cap; `None` ⇒ no cap enforced (body readers fall back
    /// to their own default via [`current_body_limit`]).
    body_limit: Option<usize>,
    /// Boot-validated response headers (security policy + optional
    /// `Server`), stamped with replace semantics — the framework value wins
    /// over a handler-set one, as `SetHeader::overriding` did.
    headers: Vec<(HeaderName, HeaderValue)>,
    /// When `true` the edge is the outermost layer (no CORS / compression
    /// configured) and runs the transport-edge problem normalizer itself —
    /// the `.around` wrap that used to carry it is not mounted at all.
    normalize: bool,
    /// When `true` a layer outside this one can replace the request body while
    /// leaving `Content-Length` describing the old one, so a declared length is
    /// no longer a bound and the edge counts the bytes instead. Set from the one
    /// knob that installs such a layer (`HttpConfig.compression`); with it off,
    /// hyper's length decoder already bounds the body and the count could never
    /// fire.
    counts_body: bool,
    /// Whether a request is filed. From `HttpConfig.access_log`; `true` by
    /// default, and the only thing this toggle decides — the correlation id is
    /// resolved, installed and echoed on every request regardless.
    access_log: bool,
    /// `HttpConfig.trusted_proxies`, shared rather than copied per request.
    /// Transport state, not the access log's: it is what decides whether an
    /// inbound `X-Forwarded-For` **or** `X-Request-Id` may be believed, and both
    /// answers are needed whether or not a line is emitted.
    trusted_proxies: Arc<[IpAddr]>,
}

impl<E> EdgeEndpoint<E> {
    pub(crate) fn new(
        inner: E,
        container: Container,
        timeout: Option<Duration>,
        body_limit: Option<usize>,
        headers: Vec<(HeaderName, HeaderValue)>,
        normalize: bool,
        counts_body: bool,
    ) -> Self {
        let shared_scope = (!container.has_dynamic_scopes())
            .then(|| Arc::new(RequestScope::new(container.clone())));
        // Read once, at boot, from the container the edge already holds — rather
        // than threaded through the transport as two more constructor
        // arguments. `ClientOrigin::of` reads the same list off the same place
        // per request; the edge cannot, because it runs *before* the request
        // scope that lookup goes through exists. An app with no `HttpConfig` —
        // an imperatively built transport, a unit test — takes the answer the
        // defaults describe: the line is on, nothing is trusted.
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
        }
    }

    /// Stamp the configured headers onto an outgoing response.
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
    /// The inner tree, under the ambient context `call` built.
    ///
    /// The context arrives already assembled rather than being opened here,
    /// because `call` installs it a second time — around the response *body*,
    /// which is written after this future has returned. **One value, installed
    /// twice**, so the two cannot come to disagree about what the request was and
    /// neither install pays for re-assembling it.
    async fn handle(
        &self,
        mut req: Request,
        continuation: &RequestContinuation,
    ) -> Result<Response> {
        // Trailing-slash normalization, before anything routes on the path.
        // `/kitchen` and `/kitchen/` are the same resource; the router is
        // exact-match, so without this the second one 404s — and a 404 leaves
        // the route's guards, interceptors and filters unrun, which makes the
        // mistake read as a broken feature rather than a typo.
        trim_trailing_slash(&mut req);

        // The URI the caller addressed, kept for the handlers that echo it back
        // (`nest_rs_http::caller_path`). This is the last point that sees it
        // whole: the router strips a global prefix off `uri()` on the way in,
        // and poem's `original_uri()` is populated by the hyper path only, so a
        // `TestApp` request would carry `/`. One refcount bump for the `Uri`
        // plus the insert a `.data()` middleware would cost anyway.
        let caller_uri = req.uri().clone();
        req.extensions_mut().insert(CallerUri(caller_uri));

        // Body cap (B-HTTP-2) — every extractor sits under it. Three answers,
        // decided by what the request declares:
        //
        // - `Content-Length` over the cap ⇒ `413` before a byte is read.
        // - An empty body ⇒ pass through: no byte exists for a cap to bound.
        //   Emptiness is a property of the body object, not the wire framing,
        //   so `TestApp` and wire traffic agree by construction.
        // - A declared length within the cap ⇒ **count what arrives**, but only
        //   where the declaration can be false. It is a claim about the wire,
        //   and a layer outside this one can invalidate it: poem's
        //   `Compression` decompresses the request body and leaves
        //   `Content-Length` describing the *compressed* bytes, so trusting it
        //   let a 64 KiB gzip body write a 64 MiB object under a 2 MiB cap.
        //   With no such layer installed, hyper's length decoder cannot yield
        //   more than the declared bytes, so the count could never fire and the
        //   wrap is pure per-request cost — hence `counts_body`, set by the
        //   transport from the one knob that installs a body-rewriting layer.
        // - No declared length (`Transfer-Encoding: chunked`, an HTTP/2+
        //   stream) ⇒ buffer up to the cap and reject past it. Buffering rather
        //   than counting, because nothing declared a length to answer with:
        //   reading it is what produces the `413`, and the cap bounds the read.
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

        // Timeout around the inner tree — guards / interceptors / handler
        // are all bounded; the edge's own header work is not. The timer arms
        // lazily: the inner future is polled once bare, and only a `Pending`
        // — a handler that actually waits on something — reaches the timer
        // wheel. A synchronous response never pays the clock read or the
        // wheel entry, with identical semantics: a timeout only ever fires
        // at an await point, so the synchronous prefix was never
        // interruptible under the eager timer either.
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
                            tracing::warn!(target: "nest_rs::http", ?timeout, "request timed out");
                            return Ok(self.finish(bare(StatusCode::GATEWAY_TIMEOUT)));
                        }
                    },
                }
            }
            None => inner.await,
        };
        // The cap answers once, here, whatever the reading extractor made of
        // the stream error — otherwise the caller's status depends on which
        // extractor happened to be reading when the count ran out.
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

    async fn call(&self, req: Request) -> Result<Response> {
        // Decided before anything else runs: everything below — the ambient
        // context, the echoed header, the access line — has to agree on one id,
        // and the only way to guarantee that is to resolve it once, here. The
        // client origin is resolved first because the id's gate reads it: one
        // proxy list, one trust decision, two consumers.
        let origin = ClientOrigin::of_with(&req, &self.trusted_proxies);
        let client = ClientIp::from(origin);
        let correlation = trace_context::resolve(&req, origin);
        let user_agent = trace_context::user_agent(&req);
        // The operation span, opened here and **not** gated on anything. It is
        // what carries `request_id` onto every event below, and what declares
        // the `actor_id` field the authn guard records into — a span that only
        // exists when an observability crate is installed makes both of those
        // optional, which is the defect this replaces.
        let span = trace_context::request_span(&req, &correlation, &client, origin, user_agent);
        // Kept because `req` is about to be consumed and the span's name needs it
        // once the router has answered — one clone of a `Method`, which is an
        // enum for every standard verb.
        let method = req.method().clone();
        // Opened before the request is consumed, because only the request can
        // answer what it was. `None` when the line is off — one `Option`, and
        // nothing else on the path pays for a disabled access log.
        let log = self
            .access_log
            .then(|| AccessLog::open(&req, correlation.clone(), client, user_agent));

        // Request scope + configured cap, ambient for everything inward —
        // guards, extractors, the data layer, global pipes — and for the
        // response body afterwards. Assembled here rather than inside `handle`
        // because a streaming body outlives that future, and what it continues
        // is *this* request, not a reconstruction of it.
        let scope = match &self.shared_scope {
            Some(shared) => Arc::clone(shared),
            None => Arc::new(RequestScope::new(self.container.clone())),
        };
        let continuation =
            RequestContinuation::new(Some(scope), correlation.clone(), self.body_limit);

        let result = self
            .handle(req, &continuation)
            .instrument(span.clone())
            .await;
        // Read here, before anything renders an `Err` into a `Response`: poem's
        // router attaches the matched template to whichever of the two came back,
        // and rendering builds a fresh response that carries none of it. This is
        // the only point where both shapes are still in hand.
        trace_context::name_route(&span, &method, matched_route(&result));
        let result = if self.normalize {
            // The transport-edge error boundary this edge absorbs when it is the
            // outermost layer: an `Err` escaping the inner tree renders without
            // the header stamp (exactly as the standalone `.around` wrap saw it),
            // then any raw transport error is lifted onto `problem+json`.
            let resp = match result {
                Ok(resp) => resp,
                Err(err) => err.into_response(),
            };
            Ok(crate::problem::normalize_error_response(resp).await)
        } else {
            result
        };

        match result {
            Ok(mut resp) => {
                // Always, whatever the access log is set to: the status is what
                // an exported server span is read for, and it costs one record.
                span.record("http.response.status_code", resp.status().as_u16());
                trace_context::stamp(&correlation, &mut resp);
                // Unconditional, unlike the log it may or may not carry: a
                // streaming body is the request still running, and what
                // `current_request_id()` answers inside one cannot depend on
                // whether an operator wanted an access line.
                Ok(response_body::carry(continuation, span, log, resp))
            }
            // Only reachable with CORS or compression configured, where a wrap
            // outside this one renders the error. The request is still filed —
            // a request that failed is exactly the one an operator looks for,
            // and it would be the only status class silently missing from the
            // log.
            Err(err) => {
                span.record("http.response.status_code", err.status().as_u16());
                if let Some(log) = log {
                    log.abandoned(err.status().as_u16());
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
            // These unit tests drive the edge directly, with nothing wrapped
            // outside it that could rewrite a body.
            false,
        )
    }

    /// The fused shape `HttpTransport` mounts when no CORS / compression is
    /// configured — the edge itself runs the problem normalizer.
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

    #[tokio::test]
    async fn installs_a_request_scope_and_forwards_the_response() {
        let ep = edge(observe_scope, None, None, Vec::new());
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status_is_ok();
        resp.assert_text("ok").await;
    }

    /// R9-5: `/kitchen` served and `/kitchen/` 404'd. The router matches
    /// exactly, so the trailing slash has to go before routing.
    #[test]
    fn a_trailing_slash_is_not_part_of_the_path() {
        assert_eq!(canonical_path("/kitchen/"), Some("/kitchen"));
        assert_eq!(canonical_path("/kitchen///"), Some("/kitchen"));
        assert_eq!(
            canonical_path("/v1/kitchen/items/"),
            Some("/v1/kitchen/items")
        );
        // `//` and friends name the root, which is spelled with one slash.
        assert_eq!(canonical_path("//"), Some("/"));
    }

    /// The no-op half: a canonical path is left exactly as it is, so the hot
    /// path neither allocates nor rebuilds the URI.
    #[test]
    fn a_canonical_path_is_left_alone() {
        assert_eq!(canonical_path("/"), None);
        assert_eq!(canonical_path("/kitchen"), None);
        assert_eq!(canonical_path(""), None);
        // An interior empty segment is a different path, not a typo to fix.
        assert_eq!(canonical_path("/kitchen//items"), None);
    }

    // The behaviour these two rules produce — the slashed form reaching the
    // route, the query surviving — is asserted through the real controller /
    // transport composition in `tests/integration/edge.rs`. Here the pure
    // function is enough.

    #[tokio::test]
    async fn declared_length_over_the_cap_is_rejected_with_headers() {
        // 413 must still carry the security headers — it used to bubble
        // through the SetHeader middleware wrapping the body-limit layer.
        // TestClient bypasses the wire, so the Content-Length hyper would
        // derive is set by hand.
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
        // No Content-Length (chunked wire traffic, or an in-process harness
        // request) forces the buffered path — the case that must read to
        // enforce the cap.
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
    async fn overrunning_handler_answers_504_with_headers() {
        let ep = edge(slow, Some(Duration::from_millis(20)), None, nosniff());
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status(StatusCode::GATEWAY_TIMEOUT);
        resp.assert_header("x-content-type-options", "nosniff");
    }

    #[tokio::test]
    async fn synchronous_response_under_a_timeout_is_forwarded() {
        // The lazy timer's fast path: a first poll that resolves never arms
        // the timer wheel — the response must come through untouched.
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
        // The lazy timer's slow path: the first poll returns `Pending`, the
        // timer arms, and a handler that finishes inside the budget still
        // resolves normally.
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
        // Replace semantics — the framework policy wins, as
        // `SetHeader::overriding` did.
        let ep = edge(sets_header, None, None, nosniff());
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status_is_ok();
        resp.assert_header("x-content-type-options", "nosniff");
    }

    #[tokio::test]
    async fn fused_normalize_lifts_a_413_onto_problem_json_with_headers() {
        // With the normalizer fused, a body-cap rejection must come out
        // exactly as it did through the standalone `.around` wrap: an
        // RFC-9457 body carrying the security headers stamped by the edge.
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
        // An `Err` escaping the inner tree renders and normalizes, but never
        // passes through `finish` — matching the standalone normalizer, which
        // sat outside the header stamp.
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
        // SetHeader forwarded errors untouched; the outer problem
        // normalizer owns their rendering. The edge must do the same.
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
