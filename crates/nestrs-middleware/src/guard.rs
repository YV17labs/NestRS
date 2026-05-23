use async_trait::async_trait;
use poem::{Endpoint, IntoResponse, Request, Response, Result};

/// A `Guard` runs before the handler and decides whether the request is
/// allowed through. Returning `Err(response)` short-circuits the chain with
/// that response — typically a 401/403/429 — so the handler never runs.
///
/// The request is borrowed **mutably**, so a guard may also *attach*
/// request-scoped context for the handler to read back via
/// [`Ctx<T>`](../../nestrs_http/struct.Ctx.html): the authenticated caller, a
/// resolved tenant, a rate-limit budget. This is the equivalent of NestJS
/// setting `request.user` in a guard.
///
/// Bind a guard to routes either globally (`HttpTransport::guard`) or
/// per-handler with `#[use_guards(MyGuard)]`, where it is resolved from the
/// container — so a guard is an ordinary `#[injectable]` provider and can
/// inject its own dependencies.
///
/// ```ignore
/// #[derive(Clone)]
/// struct Caller { api_key: String }
///
/// #[nestrs_core::injectable]
/// #[derive(Default)]
/// struct RequireApiKey;
///
/// #[async_trait::async_trait]
/// impl nestrs_middleware::Guard for RequireApiKey {
///     async fn check(&self, req: &mut poem::Request) -> Result<(), poem::Response> {
///         match req.headers().get("x-api-key").and_then(|v| v.to_str().ok()) {
///             Some(key) => {
///                 req.extensions_mut().insert(Caller { api_key: key.to_owned() });
///                 Ok(())
///             }
///             None => Err(poem::Response::builder()
///                 .status(poem::http::StatusCode::UNAUTHORIZED)
///                 .body("missing api key")),
///         }
///     }
/// }
/// ```
#[async_trait]
pub trait Guard: Send + Sync + 'static {
    async fn check(&self, req: &mut Request) -> std::result::Result<(), Response>;
}

#[async_trait]
impl<T: Guard + ?Sized> Guard for std::sync::Arc<T> {
    async fn check(&self, req: &mut Request) -> std::result::Result<(), Response> {
        (**self).check(req).await
    }
}

/// Endpoint wrapper produced by [`EndpointExt::guard`](crate::EndpointExt::guard).
pub struct GuardEndpoint<E, G> {
    inner: E,
    guard: G,
}

impl<E, G> GuardEndpoint<E, G> {
    pub fn new(inner: E, guard: G) -> Self {
        Self { inner, guard }
    }
}

impl<E, G> Endpoint for GuardEndpoint<E, G>
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
    G: Guard,
{
    type Output = Response;

    async fn call(&self, mut req: Request) -> Result<Self::Output> {
        match self.guard.check(&mut req).await {
            Ok(()) => self.inner.call(req).await.map(IntoResponse::into_response),
            Err(response) => Ok(response),
        }
    }
}
