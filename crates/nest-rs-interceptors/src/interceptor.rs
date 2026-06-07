//! The [`Interceptor`] trait — a [`Layer`] sub-trait whose impls wrap
//! endpoint execution.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use poem::{Endpoint, IntoResponse, Request, Response, Result};

/// Wraps endpoint execution: sees the request before the handler runs and the
/// response after, in a single `intercept(req, next)` call.
///
/// `Interceptor` extends [`Layer`] so the same impl can be declared at any
/// scope (global / controller / method) and the Layer System dedups by
/// [`TypeId`](std::any::TypeId).
///
/// Bind globally via [`use_interceptors_global`](crate::AppBuilderInterceptorsExt),
/// per-controller via `#[use_interceptors(...)]` on the struct, or per-handler
/// via `#[use_interceptors(...)]` beside the verb. A controller/handler
/// interceptor sits *inside* the guards — a guard runs and may short-circuit
/// before the interceptor's pre-handler work.
#[async_trait]
pub trait Interceptor: Layer {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response>;
}

#[async_trait]
impl<T: Interceptor + ?Sized> Interceptor for Arc<T> {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        (**self).intercept(req, next).await
    }
}

/// The continuation passed to an [`Interceptor`]. Call [`Next::run`] to
/// delegate to the inner endpoint (handler or next interceptor).
pub struct Next<'a> {
    inner: &'a (dyn ErasedEndpoint + Send + Sync + 'a),
}

impl<'a> Next<'a> {
    pub fn new<E>(endpoint: &'a E) -> Self
    where
        E: Endpoint + Send + Sync,
        E::Output: IntoResponse,
    {
        Self { inner: endpoint }
    }

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

pub struct InterceptorEndpoint<E, I> {
    inner: E,
    interceptor: I,
}

impl<E, I> InterceptorEndpoint<E, I> {
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
