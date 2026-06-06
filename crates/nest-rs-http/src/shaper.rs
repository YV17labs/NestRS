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

use std::future::Future;
use std::marker::PhantomData;

use poem::{Endpoint, IntoResponse, Request, Response, Result};

/// A handler wrapper keyed by a marker type. `#[routes]` applies it when a
/// handler declares a parameter of an implementing type.
pub trait RouteResponseShaper {
    /// Bits the shaper extracts from the request before the handler consumes it.
    type Captured: Send;

    fn capture(req: &Request) -> Self::Captured;

    /// Run the handler `inner` and shape its result. The shaper may wrap
    /// `inner` to install ambient state for its duration and may transform the
    /// response before returning it.
    fn run<F>(captured: Self::Captured, inner: F) -> impl Future<Output = Result<Response>> + Send
    where
        F: Future<Output = Result<Response>> + Send;
}

pub fn shaped<P, E>(inner: E, _shaper: PhantomData<P>) -> ShapedEndpoint<P, E> {
    ShapedEndpoint {
        inner,
        _marker: PhantomData,
    }
}

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
