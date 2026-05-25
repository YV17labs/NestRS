//! Per-route, type-directed response shaping.
//!
//! `#[routes]` detects a handler parameter that names a [`RouteResponseShaper`]
//! (in practice the `Authorize<_, _>` gate) and wraps the handler with
//! [`shaped`]: [`capture`](RouteResponseShaper::capture) runs against the request
//! before the handler, [`shape`](RouteResponseShaper::shape) rewrites the
//! response after. The shaper sits *inside* the route's guards, so a guard that
//! attached request context (the authorization ability) has already run when
//! `capture` reads it.
//!
//! The trait is implemented outside this crate — `nestrs-authz` shapes the body
//! to enforce field-masking — so the HTTP surface stays unaware of any specific
//! shaper. `#[routes]` emits only `::nestrs_http::shaped` plus the parameter
//! type the app already wrote, never a path into the implementing crate.

use std::future::Future;
use std::marker::PhantomData;

use poem::{Endpoint, IntoResponse, Request, Response, Result};

/// A response transform keyed by a marker type `Self`. The `#[routes]` macro
/// applies it when a handler declares a parameter of an implementing type.
pub trait RouteResponseShaper {
    /// What [`capture`](Self::capture) extracts from the request for
    /// [`shape`](Self::shape) to use after the handler runs (the request is
    /// consumed by the handler, so anything `shape` needs is taken here).
    type Captured: Send;

    fn capture(req: &Request) -> Self::Captured;

    fn shape(captured: Self::Captured, resp: Response)
        -> impl Future<Output = Response> + Send;
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
        let resp = self.inner.call(req).await?.into_response();
        Ok(P::shape(captured, resp).await)
    }
}
