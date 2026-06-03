//! HTTP binding for request-scoped providers — [`RequestScopeEndpoint`] installs
//! a fresh [`RequestScope`] per request; [`Scoped<T>`] reads it back to resolve
//! an `#[injectable(scope = request)]` provider (or, falling through, a
//! singleton — prefer plain `#[inject]` for those).

use std::any::type_name;
use std::ops::Deref;
use std::sync::Arc;

use nestrs_core::{Container, RequestScope};
use poem::http::StatusCode;
use poem::{Endpoint, Error, FromRequest, IntoResponse, Request, RequestBody, Response, Result};

/// Installs a fresh [`RequestScope`] (over the singleton container) into each
/// request's extensions before delegating inward, so guards and handlers can
/// resolve request-scoped providers via [`Scoped<T>`]. Applied outermost by
/// [`HttpTransport`](crate::HttpTransport).
pub struct RequestScopeEndpoint<E> {
    inner: E,
    container: Container,
}

impl<E> RequestScopeEndpoint<E> {
    pub fn new(inner: E, container: Container) -> Self {
        Self { inner, container }
    }
}

impl<E> Endpoint for RequestScopeEndpoint<E>
where
    E: Endpoint,
    E::Output: IntoResponse,
{
    type Output = Response;

    async fn call(&self, mut req: Request) -> Result<Self::Output> {
        req.extensions_mut()
            .insert(Arc::new(RequestScope::new(self.container.clone())));
        self.inner.call(req).await.map(IntoResponse::into_response)
    }
}

/// Resolves a provider of type `T` from the current request's
/// [`RequestScope`]. Rejects with `500` if the scope is absent (a transport
/// wiring bug) or if no provider is registered for `T`.
pub struct Scoped<T>(pub Arc<T>);

impl<T> Scoped<T> {
    pub fn into_inner(self) -> Arc<T> {
        self.0
    }
}

impl<T> Deref for Scoped<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<'a, T: Send + Sync + 'static> FromRequest<'a> for Scoped<T> {
    async fn from_request(req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        let scope = req.extensions().get::<Arc<RequestScope>>().ok_or_else(|| {
            Error::from_string(
                "request scope not installed — RequestScopeEndpoint must wrap the route tree",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        match scope.get::<T>() {
            Some(value) => Ok(Scoped(value)),
            None => Err(Error::from_string(
                format!(
                    "no provider registered for `{}` — add it to a module's providers",
                    type_name::<T>()
                ),
                StatusCode::INTERNAL_SERVER_ERROR,
            )),
        }
    }
}
