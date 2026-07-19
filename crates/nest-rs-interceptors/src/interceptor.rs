//! The [`Interceptor`] trait — a [`Layer`] sub-trait whose impls wrap
//! handler execution across every transport (HTTP, GraphQL, WS).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use poem::{Endpoint, IntoResponse, Request, Response, Result};

/// Wraps handler execution. An [`Interceptor`] sees the inputs before the
/// handler runs and the outputs after. `intercept(req, next)` is the HTTP
/// entry. A GraphQL `POST` and a WS upgrade are HTTP requests, so a *global*
/// interceptor covers them through the transport-edge wrap; per-resolver /
/// per-message wrapping is not offered (a former reserved seam was removed
/// until it is actually wired).
///
/// `Interceptor` extends [`Layer`] so the same impl can be declared at any
/// scope (global / controller / method) and the Layer System dedups by
/// [`TypeId`](std::any::TypeId).
///
/// Bind globally via [`use_interceptors_global`](crate::AppBuilderInterceptorsExt),
/// per-provider via `#[use_interceptors(...)]` on the
/// controller/resolver/gateway, or per-handler beside the verb /
/// `#[query]` / `#[subscribe_message]`.
#[async_trait]
pub trait Interceptor: Layer {
    /// HTTP entry. The per-route shaper calls this once for every HTTP
    /// route. Required (no default) so an `Interceptor` impl that
    /// genuinely targets HTTP cannot forget to wire it.
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response>;
}

#[async_trait]
impl<T: Interceptor + ?Sized> Interceptor for Arc<T> {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        (**self).intercept(req, next).await
    }
}

/// The continuation passed to an HTTP [`Interceptor::intercept`]. Call
/// [`Next::run`] to delegate to the inner endpoint (handler or next
/// interceptor).
pub struct Next<'a> {
    inner: &'a (dyn ErasedEndpoint + Send + Sync + 'a),
}

impl<'a> Next<'a> {
    /// Build a continuation over `endpoint` — the next link an interceptor may
    /// delegate to.
    pub fn new<E>(endpoint: &'a E) -> Self
    where
        E: Endpoint + Send + Sync,
        E::Output: IntoResponse,
    {
        Self { inner: endpoint }
    }

    /// Delegate to the inner endpoint (handler or next interceptor) with `req`.
    pub async fn run(self, req: Request) -> Result<Response> {
        self.inner.call_boxed(req).await
    }
}

/// Type-erased view of any `Endpoint<Output: IntoResponse>`. Lets [`Next`]
/// hold any inner endpoint without leaking the concrete `E` generic across
/// the [`Interceptor`] trait (which would force every impl to also be generic).
trait ErasedEndpoint {
    fn call_boxed<'a>(
        &'a self,
        req: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response>> + Send + 'a>>;
}

impl<E> ErasedEndpoint for E
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
{
    fn call_boxed<'a>(
        &'a self,
        req: Request,
    ) -> Pin<Box<dyn Future<Output = Result<Response>> + Send + 'a>> {
        Box::pin(async move { self.call(req).await.map(IntoResponse::into_response) })
    }
}

/// A poem endpoint `E` wrapped by interceptor `I`, produced by
/// [`InterceptorExt::interceptor`](crate::InterceptorExt::interceptor).
pub struct InterceptorEndpoint<E, I> {
    inner: E,
    interceptor: I,
}

impl<E, I> InterceptorEndpoint<E, I> {
    /// Pair `inner` with `interceptor` so the interceptor runs around each call.
    pub fn new(inner: E, interceptor: I) -> Self {
        Self { inner, interceptor }
    }
}

impl<E, I> Endpoint for InterceptorEndpoint<E, I>
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
    I: Interceptor,
{
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Self::Output> {
        let next = Next::new(&self.inner);
        self.interceptor.intercept(req, next).await
    }
}

#[cfg(test)]
mod tests {
    use poem::handler;
    use poem::http::StatusCode;

    use super::*;

    struct Stamp;

    impl Layer for Stamp {}

    #[async_trait]
    impl Interceptor for Stamp {
        async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
            let mut resp = next.run(req).await?;
            resp.headers_mut()
                .insert("x-stamp", "hit".parse().expect("valid header value"));
            Ok(resp)
        }
    }

    struct ShortCircuit;

    impl Layer for ShortCircuit {}

    #[async_trait]
    impl Interceptor for ShortCircuit {
        async fn intercept(&self, _req: Request, _next: Next<'_>) -> Result<Response> {
            Ok(Response::builder()
                .status(StatusCode::FORBIDDEN)
                .body("blocked"))
        }
    }

    #[handler]
    fn ok_handler() -> &'static str {
        "ok"
    }

    #[tokio::test]
    async fn the_interceptor_wraps_the_inner_endpoint() {
        let ep = InterceptorEndpoint::new(ok_handler, Stamp);
        let resp = ep.call(Request::default()).await.expect("handler runs");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("x-stamp").map(|v| v.as_bytes()),
            Some(&b"hit"[..])
        );
    }

    #[tokio::test]
    async fn an_interceptor_may_short_circuit_without_running_the_handler() {
        let ep = InterceptorEndpoint::new(ok_handler, ShortCircuit);
        let resp = ep.call(Request::default()).await.expect("short circuit");
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
