//! The [`Filter`] trait — extends [`Layer`] for the Layer System.

use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use poem::http::{HeaderMap, Method, Uri};
use poem::{Endpoint, IntoResponse, Request, Response, Result};

/// Read-only view of the request handed to a [`Filter`]. The original
/// `poem::Request` has been consumed by the inner endpoint by the time the
/// filter runs (and is not `Clone`), so the routing-relevant bits are
/// captured up front.
#[derive(Debug, Clone)]
pub struct RequestSnapshot {
    /// The request method.
    pub method: Method,
    /// The request URI (path + query).
    pub uri: Uri,
    /// The request headers.
    pub headers: HeaderMap,
}

impl RequestSnapshot {
    /// Capture the routing-relevant parts of `req` before the inner endpoint
    /// consumes it.
    pub fn from_req(req: &Request) -> Self {
        Self {
            method: req.method().clone(),
            uri: req.uri().clone(),
            headers: req.headers().clone(),
        }
    }
}

/// Maps errors returned by the inner handler to a response. Runs only on the
/// error path; successful results pass through unchanged. A global filter
/// covers a GraphQL `POST` or WS upgrade through its HTTP entry — there is no
/// per-resolver / per-message seam (former reserved ones were removed until
/// they are actually wired).
///
/// `Filter` extends [`Layer`] so global + per-scope declarations dedup by
/// [`TypeId`](std::any::TypeId).
#[async_trait]
pub trait Filter: Layer {
    /// HTTP entry — required, no default: a filter that targets HTTP
    /// without implementing this would silently let errors through.
    async fn filter(&self, req: &RequestSnapshot, error: poem::Error) -> Response;
}

#[async_trait]
impl<T: Filter + ?Sized> Filter for Arc<T> {
    async fn filter(&self, req: &RequestSnapshot, error: poem::Error) -> Response {
        (**self).filter(req, error).await
    }
}

/// A poem endpoint `E` wrapped by filter `F`, produced by
/// [`FilterExt::filter`](crate::FilterExt::filter).
pub struct FilterEndpoint<E, F> {
    inner: E,
    filter: F,
}

impl<E, F> FilterEndpoint<E, F> {
    /// Pair `inner` with `filter` so the filter maps errors it returns.
    pub fn new(inner: E, filter: F) -> Self {
        Self { inner, filter }
    }
}

impl<E, F> Endpoint for FilterEndpoint<E, F>
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
    F: Filter,
{
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Self::Output> {
        let snapshot = RequestSnapshot::from_req(&req);
        match self.inner.call(req).await {
            Ok(out) => Ok(out.into_response()),
            Err(err) => {
                let mut resp = self.filter.filter(&snapshot, err).await;
                // The handler failed; this response only shapes the client
                // answer. Tag it so the ambient transaction rolls back even
                // when the mapped status reads as success.
                resp.extensions_mut().insert(nest_rs_core::MappedError);
                Ok(resp)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use poem::http::StatusCode;
    use poem::{Endpoint, endpoint::make, handler};

    use super::*;

    struct TeapotFilter;

    impl Layer for TeapotFilter {}

    #[async_trait]
    impl Filter for TeapotFilter {
        async fn filter(&self, _req: &RequestSnapshot, _error: poem::Error) -> Response {
            Response::builder()
                .status(StatusCode::IM_A_TEAPOT)
                .body("mapped")
        }
    }

    #[handler]
    fn ok_handler() -> &'static str {
        "ok"
    }

    #[tokio::test]
    async fn success_passes_through_unmapped() {
        let ep = FilterEndpoint::new(ok_handler, TeapotFilter);
        let resp = ep
            .call(Request::default())
            .await
            .expect("success flows through");
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.extensions()
                .get::<nest_rs_core::MappedError>()
                .is_none()
        );
    }

    #[tokio::test]
    async fn errors_map_to_the_filters_response_tagged_mapped_error() {
        let failing = make(|_req: Request| async {
            Err::<Response, _>(poem::Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
        });
        let ep = FilterEndpoint::new(failing, TeapotFilter);
        let resp = ep.call(Request::default()).await.expect("error is mapped");
        assert_eq!(resp.status(), StatusCode::IM_A_TEAPOT);
        assert!(
            resp.extensions()
                .get::<nest_rs_core::MappedError>()
                .is_some(),
            "a mapped error response must carry the rollback tag",
        );
    }
}
