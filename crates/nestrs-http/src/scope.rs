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

#[cfg(test)]
mod tests {
    use super::*;
    use poem::Body;
    use poem::handler;

    struct Marker(&'static str);

    #[test]
    fn scoped_into_inner_yields_the_arc() {
        let value: Arc<Marker> = Arc::new(Marker("hi"));
        let scoped = Scoped(value.clone());
        let inner = scoped.into_inner();
        assert!(Arc::ptr_eq(&inner, &value));
    }

    #[test]
    fn scoped_deref_borrows_the_inner_value() {
        let scoped = Scoped(Arc::new(Marker("bye")));
        assert_eq!(scoped.0.as_ref().0, "bye");
        // Deref reaches the field through `&*scoped`.
        assert_eq!((*scoped).0, "bye");
    }

    #[handler]
    async fn observe(req: &Request) -> &'static str {
        assert!(
            req.extensions().get::<Arc<RequestScope>>().is_some(),
            "RequestScopeEndpoint installed an Arc<RequestScope> per request",
        );
        "ok"
    }

    #[tokio::test]
    async fn endpoint_installs_a_request_scope_into_the_request_extensions() {
        let container = Container::builder().build();
        let endpoint = RequestScopeEndpoint::new(observe, container);

        let req = Request::builder().body(Body::empty());
        let resp = endpoint.call(req).await.expect("handler runs");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn endpoint_propagates_the_inner_response_unchanged() {
        // `IntoResponse::into_response` is invoked on the inner endpoint output —
        // a plain `&str` becomes a 200 with that body.
        let container = Container::builder().build();
        let endpoint = RequestScopeEndpoint::new(observe, container);

        let resp = endpoint
            .call(Request::builder().body(Body::empty()))
            .await
            .expect("ok");
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        assert_eq!(bytes.as_ref(), b"ok");
    }

    #[tokio::test]
    async fn scoped_from_request_resolves_a_registered_provider() {
        // A singleton falls through `RequestScope::get`, the documented escape
        // hatch for `Scoped<T>` when no scoped factory exists for `T`.
        let container = Container::builder().provide(Marker("registered")).build();
        let scope = Arc::new(RequestScope::new(container));

        let mut req = Request::default();
        req.extensions_mut().insert(scope);
        let (req, mut body) = req.split();

        let scoped: Scoped<Marker> = Scoped::from_request(&req, &mut body)
            .await
            .expect("resolves via singleton fallback");
        assert_eq!(scoped.0.0, "registered");
    }

    #[tokio::test]
    async fn scoped_from_request_returns_500_when_no_scope_is_installed() {
        let req = Request::default();
        let (req, mut body) = req.split();

        let err = match Scoped::<Marker>::from_request(&req, &mut body).await {
            Ok(_) => panic!("no Arc<RequestScope> in extensions should reject"),
            Err(e) => e,
        };
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("request scope not installed"),
            "diagnostic mentions the wiring bug: {text}",
        );
    }

    #[tokio::test]
    async fn scoped_from_request_returns_500_when_no_provider_is_registered() {
        // Scope installed but `Marker` was never provided.
        let container = Container::builder().build();
        let scope = Arc::new(RequestScope::new(container));

        let mut req = Request::default();
        req.extensions_mut().insert(scope);
        let (req, mut body) = req.split();

        let err = match Scoped::<Marker>::from_request(&req, &mut body).await {
            Ok(_) => panic!("no provider for Marker should reject"),
            Err(e) => e,
        };
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = resp.into_body().into_bytes().await.expect("body");
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("no provider registered for"),
            "diagnostic names the missing type: {text}",
        );
        assert!(text.contains("Marker"), "the type name surfaces: {text}");
    }
}
