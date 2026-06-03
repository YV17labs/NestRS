use async_trait::async_trait;
use poem::{Endpoint, IntoResponse, Request, Response, Result};

/// Pre-handler authorization. Returning `Err(response)` short-circuits the
/// chain with that response (typically 401/403/429); the handler never runs.
///
/// The request is borrowed **mutably**, so a guard may also *attach*
/// request-scoped context the handler reads back via
/// [`Ctx<T>`](../../nestrs_http/struct.Ctx.html) — e.g. attaching the
/// authenticated principal for the handler to read back.
///
/// Bind globally (`HttpTransport::guard`) or per-handler with
/// `#[use_guards(MyGuard)]`, where the guard is resolved from the container as
/// any `#[injectable]` provider.
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
