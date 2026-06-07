//! Poem endpoint adapter that wraps any [`Guard`] as the HTTP-side check.
//!
//! Used by the global `HttpTransport::guard(...)` registration and by
//! gateway-struct `#[use_guards(...)]` for the WebSocket upgrade (which is an
//! HTTP GET). The adapter calls [`Guard::check_http`] and converts a
//! [`Denial`](crate::Denial) to a poem [`Response`].

use std::sync::Arc;

use poem::{Endpoint, IntoResponse, Request, Response, Result};

use crate::Guard;
use crate::integration::denial_to_http_response;

/// Wraps any poem endpoint with a [`Guard`]'s `check_http` step.
pub struct GuardEndpoint<E, G: ?Sized> {
    inner: E,
    guard: Arc<G>,
}

impl<E, G: ?Sized> GuardEndpoint<E, G> {
    pub fn new(inner: E, guard: Arc<G>) -> Self {
        Self { inner, guard }
    }
}

impl<E, G> Endpoint for GuardEndpoint<E, G>
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
    G: Guard + ?Sized,
{
    type Output = Response;

    async fn call(&self, mut req: Request) -> Result<Self::Output> {
        match self.guard.check_http(&mut req).await {
            Ok(()) => self.inner.call(req).await.map(IntoResponse::into_response),
            Err(denial) => Ok(denial_to_http_response(denial)),
        }
    }
}

/// Extension trait so a poem endpoint can chain `.guard(arc_guard)`.
///
/// Bring into scope where the WS gateway or HTTP transport macros emit
/// `.guard(...)` for gateway-/transport-level guards. The chain runs the
/// [`Guard::check_http`] step before delegating to the inner endpoint.
pub trait GuardExt: Endpoint + Sized + Send + Sync
where
    Self::Output: IntoResponse,
{
    fn guard<G>(self, guard: Arc<G>) -> GuardEndpoint<Self, G>
    where
        G: Guard + ?Sized,
    {
        GuardEndpoint::new(self, guard)
    }
}

impl<E> GuardExt for E
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
{
}
