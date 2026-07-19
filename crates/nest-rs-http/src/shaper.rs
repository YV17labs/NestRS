//! Per-route, type-directed response shaping.
//!
//! `#[routes]` detects a handler parameter naming a [`RouteResponseShaper`]
//! (in practice the `Authorize<_, _>` gate) and wraps the handler with
//! [`shaped`]. The shaper sits *inside* the route's guards — a guard that
//! attached request context (the authorization ability) has already run when
//! `capture` reads it. `run` then wraps the handler future, so the shaper may
//! both install ambient state around the handler and rewrite its response.
//!
//! The trait is implemented outside this crate (`nest_rs_authz::http`) so the
//! HTTP surface stays unaware of any specific shaper.

use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;

use poem::http::StatusCode;
use poem::{Endpoint, Error, IntoResponse, Request, Response, Result};

/// A handler wrapper keyed by a marker type. `#[routes]` applies it when a
/// handler declares a parameter of an implementing type.
pub trait RouteResponseShaper {
    /// Bits the shaper extracts from the request before the handler consumes it.
    type Captured: Send;

    /// Snapshot what the shaper needs off the request (e.g. the ambient
    /// ability) before the handler takes ownership of it.
    fn capture(req: &Request) -> Self::Captured;

    /// Run the handler `inner` and shape its result. The shaper may wrap
    /// `inner` to install ambient state for its duration and may transform the
    /// response before returning it.
    fn run<F>(captured: Self::Captured, inner: F) -> impl Future<Output = Result<Response>> + Send
    where
        F: Future<Output = Result<Response>> + Send;
}

/// Wrap `inner` in the shaper `P`. Emitted by `#[routes]` when a handler
/// declares a parameter of a [`RouteResponseShaper`] type.
pub fn shaped<P, E>(inner: E, _shaper: PhantomData<P>) -> ShapedEndpoint<P, E> {
    ShapedEndpoint {
        inner,
        _marker: PhantomData,
    }
}

/// An endpoint wrapped by the [`RouteResponseShaper`] `P`, applying `capture`
/// before and `run` around the inner handler.
pub struct ShapedEndpoint<P, E> {
    inner: E,
    _marker: PhantomData<fn() -> P>,
}

impl<P, E> Endpoint for ShapedEndpoint<P, E>
where
    P: RouteResponseShaper + Send + Sync + 'static,
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
{
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Response> {
        let captured = P::capture(&req);
        let inner = async move { self.inner.call(req).await.map(IntoResponse::into_response) };
        P::run(captured, inner).await
    }
}

tokio::task_local! {
    /// The per-request masking probe (see [`MaskProbe`]). A task-local `Cell`
    /// instead of a request extension: zero heap allocation per request, and
    /// extractors run inside the endpoint's own task so the scope provably
    /// covers them — the same pattern as the ambient executor and ability.
    static MASK_PROBE: Cell<bool>;
}

/// The run-time cross-check for the masking arm. `#[routes]` detects the
/// masking extractors (`Authorize<_, _>`, `Bind<_, _>`) **by path-segment
/// name** to arm the response shaper; a renamed import (`use Authorize as Az`)
/// escapes that detection, leaving the route's response unmasked while the
/// extractor still gates it. The probe closes that hole: routes the macro did
/// **not** arm run inside a probe scope ([`MaskProbedEndpoint`]), and a
/// [`mark`](MaskProbe::mark) from an extractor on a success response fails
/// the request closed instead of shipping unmasked fields.
pub struct MaskProbe;

impl MaskProbe {
    /// Record that a masking extractor ran on this request. Called by the
    /// extractors themselves; outside a probe scope (an armed route, another
    /// transport) this is a no-op.
    pub fn mark() {
        let _ = MASK_PROBE.try_with(|marked| marked.set(true));
    }
}

/// Wrap an **unarmed** route with the masking cross-check. Emitted by
/// `#[routes]` for every handler whose parameter list did not arm the response
/// shaper.
pub fn mask_probed<E>(inner: E, route: &'static str) -> MaskProbedEndpoint<E> {
    MaskProbedEndpoint { inner, route }
}

/// See [`MaskProbe`]: fails a success response closed when a masking extractor
/// ran on a route whose response shaper is not armed.
pub struct MaskProbedEndpoint<E> {
    inner: E,
    route: &'static str,
}

impl<E> Endpoint for MaskProbedEndpoint<E>
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
{
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Response> {
        let (marked, resp) = MASK_PROBE
            .scope(Cell::new(false), async {
                let resp = self.inner.call(req).await.map(IntoResponse::into_response);
                (MASK_PROBE.with(Cell::get), resp)
            })
            .await;
        let resp = resp?;
        if marked && resp.status().is_success() {
            tracing::error!(
                target: "nest_rs::http",
                route = self.route,
                "a masking extractor ran but response masking is not armed on this route — \
                 spell `Authorize<..>`/`Bind<..>` with their own names (a renamed import \
                 escapes `#[routes]` detection); failing closed",
            );
            return Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR));
        }
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use poem::handler;
    use poem::test::TestClient;

    use super::*;

    #[handler]
    fn marks_and_succeeds() -> &'static str {
        MaskProbe::mark();
        "unmasked body"
    }

    #[handler]
    fn plain() -> &'static str {
        "ok"
    }

    #[tokio::test]
    async fn a_marked_probe_on_success_fails_closed() {
        let ep = mask_probed(marks_and_succeeds, "GET /users");
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn an_unmarked_probe_passes_the_response_through() {
        let ep = mask_probed(plain, "GET /health");
        let resp = TestClient::new(ep).get("/").send().await;
        resp.assert_status_is_ok();
        resp.assert_text("ok").await;
    }
}
