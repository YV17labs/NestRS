use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use poem::{Endpoint, IntoResponse, Request, Response, Result};

/// An `Interceptor` wraps endpoint execution: it sees the request before the
/// handler runs and the response after, in a single async function.
///
/// Use for cross-cutting concerns: logging, metrics, timing,
/// response-shaping. Equivalent to a Poem `Middleware` but with a clearer
/// `intercept(req, next)` signature.
///
/// Bind an interceptor at three levels, mirroring NestJS's `@UseInterceptors`:
/// globally (`HttpTransport::interceptor`, or `#[interceptor]` for zero-config
/// auto-discovery), per-controller (`#[use_interceptors(...)]` on the struct), or
/// per-handler (`#[use_interceptors(...)]` beside the verb attribute). A
/// controller/handler interceptor is a plain `#[injectable] + impl Interceptor`
/// resolved from the container per route, and sits *inside* the guards — so a
/// guard runs (and may short-circuit) before the interceptor's pre-handler work.
///
/// ```ignore
/// struct LogTiming;
///
/// #[async_trait::async_trait]
/// impl nestrs_middleware::Interceptor for LogTiming {
///     async fn intercept(
///         &self,
///         req: Request,
///         next: nestrs_middleware::Next<'_>,
///     ) -> poem::Result<Response> {
///         let start = std::time::Instant::now();
///         let res = next.run(req).await;
///         tracing::info!(elapsed_ms = start.elapsed().as_millis());
///         res
///     }
/// }
/// ```
#[async_trait]
pub trait Interceptor: Send + Sync + 'static {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response>;
}

#[async_trait]
impl<T: Interceptor + ?Sized> Interceptor for std::sync::Arc<T> {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        (**self).intercept(req, next).await
    }
}

/// The continuation passed to an [`Interceptor`]. Call [`Next::run`] to
/// delegate to the inner endpoint (handler or the next interceptor in the
/// chain).
pub struct Next<'a> {
    inner: &'a (dyn ErasedEndpoint + Send + Sync + 'a),
}

impl<'a> Next<'a> {
    pub(crate) fn new<E>(endpoint: &'a E) -> Self
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

/// Type-erased view of any `Endpoint<Output: IntoResponse>`. Used so [`Next`]
/// can hold a reference to *any* inner endpoint without leaking the concrete
/// `E` generic across the [`Interceptor`] trait — that would force every
/// interceptor impl to also be generic.
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

/// Endpoint wrapper produced by [`EndpointExt::interceptor`](crate::EndpointExt::interceptor).
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
