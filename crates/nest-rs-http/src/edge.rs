//! The fused transport-edge endpoint — one layer carrying the per-request
//! concerns every route shares: request-scope install, body-size cap,
//! request timeout, and the default response headers (security + `Server`).
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
use std::pin::pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use nest_rs_core::{Container, RequestScope, with_request_scope};
use poem::error::ReadBodyError;
use poem::http::{HeaderName, HeaderValue, StatusCode};
use poem::web::headers::{ContentLength, HeaderMapExt};
use poem::{Endpoint, IntoResponse, Request, Response, Result};

/// A bare status-only response — the edge's own rejections (`413`, `504`).
fn bare(status: StatusCode) -> Response {
    Response::builder().status(status).finish()
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
}

impl<E> EdgeEndpoint<E> {
    pub(crate) fn new(
        inner: E,
        container: Container,
        timeout: Option<Duration>,
        body_limit: Option<usize>,
        headers: Vec<(HeaderName, HeaderValue)>,
        normalize: bool,
    ) -> Self {
        let shared_scope = (!container.has_dynamic_scopes())
            .then(|| Arc::new(RequestScope::new(container.clone())));
        Self {
            inner,
            container,
            shared_scope,
            timeout,
            body_limit,
            headers,
            normalize,
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
    async fn handle(&self, mut req: Request) -> Result<Response> {
        // Body cap (B-HTTP-2) — every extractor sits under it. Four cases,
        // cheapest first:
        //
        // 1. `Content-Length` over the cap ⇒ `413` before a byte is read.
        // 2. Declared length within the cap ⇒ pass the body through
        //    untouched — the framing already bounds it.
        // 3. No declared length but a size hint of exactly zero (a bodyless
        //    GET, an explicit `Body::empty()`) ⇒ pass through: no byte
        //    exists for a cap to bound. Emptiness is a property of the body
        //    object, not the wire framing, so `TestApp` and wire traffic
        //    agree by construction.
        // 4. No declared length and possibly non-empty (`Transfer-Encoding:
        //    chunked`, an HTTP/2+ stream) ⇒ buffer up to the cap and reject
        //    past it.
        if let Some(limit) = self.body_limit {
            if let Some(ContentLength(declared)) = req.headers().typed_get::<ContentLength>() {
                if declared as usize > limit {
                    return Ok(self.finish(bare(StatusCode::PAYLOAD_TOO_LARGE)));
                }
            } else {
                let body = req.take_body();
                if body.is_empty() {
                    req.set_body(body);
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
        }

        // Request scope + configured cap, ambient for everything inward —
        // guards, extractors, the data layer, global pipes.
        let scope = match &self.shared_scope {
            Some(shared) => Arc::clone(shared),
            None => Arc::new(RequestScope::new(self.container.clone())),
        };

        // Timeout around the inner tree — guards / interceptors / handler
        // are all bounded; the edge's own header work is not. The timer arms
        // lazily: the inner future is polled once bare, and only a `Pending`
        // — a handler that actually waits on something — reaches the timer
        // wheel. A synchronous response never pays the clock read or the
        // wheel entry, with identical semantics: a timeout only ever fires
        // at an await point, so the synchronous prefix was never
        // interruptible under the eager timer either.
        let inner = with_request_scope(scope, self.body_limit, self.inner.call(req));
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
        let result = self.handle(req).await;
        if !self.normalize {
            return result;
        }
        // The transport-edge error boundary this edge absorbs when it is the
        // outermost layer: an `Err` escaping the inner tree renders without
        // the header stamp (exactly as the standalone `.around` wrap saw it),
        // then any raw transport error is lifted onto `problem+json`.
        let resp = match result {
            Ok(resp) => resp,
            Err(err) => err.into_response(),
        };
        Ok(crate::problem::normalize_error_response(resp).await)
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
