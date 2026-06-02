//! Per-route, type-directed response shaping.
//!
//! `#[routes]` detects a handler parameter that names a [`RouteResponseShaper`]
//! (in practice the `Authorize<_, _>` gate) and wraps the handler with
//! [`shaped`]: [`capture`](RouteResponseShaper::capture) runs against the request
//! before the handler, then [`run`](RouteResponseShaper::run) **wraps the handler
//! future** — so the shaper can both install ambient request state around the
//! handler (the caller's authorization ability, read by the data layer) and
//! rewrite the response after. The shaper sits *inside* the route's guards, so a
//! guard that attached request context (the authorization ability) has already
//! run when `capture` reads it.
//!
//! The trait is implemented outside this crate — `nestrs_authz::http` installs
//! the ambient ability and masks the body — so the HTTP surface stays unaware
//! of any specific shaper. `#[routes]` emits only `::nestrs_http::shaped` plus
//! the parameter type the app already wrote, never a path into the
//! implementing crate.

use std::future::Future;
use std::marker::PhantomData;

use poem::{Endpoint, IntoResponse, Request, Response, Result};

/// A handler wrapper keyed by a marker type `Self`. The `#[routes]` macro applies
/// it when a handler declares a parameter of an implementing type.
pub trait RouteResponseShaper {
    /// What [`capture`](Self::capture) extracts from the request for
    /// [`run`](Self::run) to use (the request is consumed by the handler, so
    /// anything `run` needs from it is taken here, before).
    type Captured: Send;

    fn capture(req: &Request) -> Self::Captured;

    /// Run the handler `inner` and shape its result. The shaper may wrap `inner`
    /// to install ambient state for the duration of the handler (e.g. the caller's
    /// ability) and may transform the response before returning it.
    fn run<F>(captured: Self::Captured, inner: F) -> impl Future<Output = Result<Response>> + Send
    where
        F: Future<Output = Result<Response>> + Send;
}

/// Wrap `inner` so the shaper `P` transforms its response. `P` is named via
/// `PhantomData` so a caller (the `#[routes]` macro) can pick the marker type
/// without a value of it.
pub fn shaped<P, E>(inner: E, _shaper: PhantomData<P>) -> ShapedEndpoint<P, E> {
    ShapedEndpoint {
        inner,
        _marker: PhantomData,
    }
}

/// Endpoint produced by [`shaped`].
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
