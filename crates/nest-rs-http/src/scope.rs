//! HTTP binding for request-scoped providers: [`Scoped<T>`] resolves an
//! `#[injectable(scope = request)]` provider from the scope the transport edge
//! installs per request.

use std::any::type_name;
use std::ops::Deref;
use std::sync::Arc;

use poem::http::StatusCode;
use poem::{Error, FromRequest, Request, RequestBody, Result};

/// Resolves a provider of type `T` from the current request's
/// [`RequestScope`](nest_rs_core::RequestScope) the transport edge installed.
/// Rejects with `500` if the scope is absent or no provider is registered for `T`.
pub struct Scoped<T>(pub Arc<T>);

impl<T> Scoped<T> {
    /// Take the resolved provider handle out of the extractor.
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
    async fn from_request(_req: &'a Request, _body: &mut RequestBody) -> Result<Self> {
        let scope = crate::current_request_scope().ok_or_else(|| {
            Error::from_string(
                "request scope not installed — the transport edge must wrap the route tree",
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
    use nest_rs_core::{Container, RequestScope};

    use super::*;

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
        assert_eq!((*scoped).0, "bye");
    }

    #[tokio::test]
    async fn scoped_from_request_resolves_a_registered_provider() {
        // A singleton falls through `RequestScope::get`.
        let container = Container::builder().provide(Marker("registered")).build();
        let scope = Arc::new(RequestScope::new(container));

        let (req, mut body) = Request::default().split();
        let scoped: Scoped<Marker> = crate::with_request_scope(
            Some(scope),
            nest_rs_core::Correlation::minted(None),
            Scoped::from_request(&req, &mut body),
        )
        .await
        .expect("resolves via singleton fallback");
        assert_eq!(scoped.0.0, "registered");
    }

    #[tokio::test]
    async fn scoped_from_request_returns_500_when_no_scope_is_installed() {
        let req = Request::default();
        let (req, mut body) = req.split();

        let err = match Scoped::<Marker>::from_request(&req, &mut body).await {
            Ok(_) => panic!("no ambient request scope should reject"),
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
        let container = Container::builder().build();
        let scope = Arc::new(RequestScope::new(container));

        let (req, mut body) = Request::default().split();
        let err = match crate::with_request_scope(
            Some(scope),
            nest_rs_core::Correlation::minted(None),
            Scoped::<Marker>::from_request(&req, &mut body),
        )
        .await
        {
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
