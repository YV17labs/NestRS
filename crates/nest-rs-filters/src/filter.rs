//! The [`Filter`] trait — extends [`Layer`] for the Layer System.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_core::layer_chain::ResolvedLayer;
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

// Manual forward, not `#[async_trait]`: the macro would wrap the inner
// (already boxed) future in a second box, taxing every call made through an
// `Arc<dyn Filter>` without `.as_ref()`.
impl<T: Filter + ?Sized> Filter for Arc<T> {
    fn filter<'s, 'r, 'fut>(
        &'s self,
        req: &'r RequestSnapshot,
        error: poem::Error,
    ) -> Pin<Box<dyn Future<Output = Response> + Send + 'fut>>
    where
        's: 'fut,
        'r: 'fut,
        Self: 'fut,
    {
        (**self).filter(req, error)
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

/// A poem endpoint wrapped by a whole filter chain in one endpoint, replacing
/// one nested [`FilterEndpoint`] per entry at the composition sites
/// (per-route and transport edge).
///
/// Equivalent to the nesting it replaces: the request snapshot is captured
/// once (between two directly-nested filters nothing rewrites the request, so
/// every level of the old nesting captured the same state), and an error is
/// offered to the entries innermost-first — [`Filter::filter`] is infallible,
/// so whichever entry maps it first turns the walk into a pass-through,
/// exactly as the nested endpoints behaved. On the success path the chain
/// costs the one snapshot and nothing per entry.
pub struct FilterChain<E> {
    chain: Vec<ResolvedLayer<dyn Filter>>,
    inner: E,
}

impl<E> FilterChain<E> {
    /// Wrap `inner` in `chain`, ordered outermost-first (first listed =
    /// outermost on the error path).
    pub fn new(inner: E, chain: Vec<ResolvedLayer<dyn Filter>>) -> Self {
        Self { chain, inner }
    }
}

impl<E> Endpoint for FilterChain<E>
where
    E: Endpoint + Send + Sync,
    E::Output: IntoResponse,
{
    type Output = Response;

    async fn call(&self, req: Request) -> Result<Self::Output> {
        // An empty chain (a composed stack whose filter stage is unused)
        // must not pay the snapshot capture.
        if self.chain.is_empty() {
            return self.inner.call(req).await.map(IntoResponse::into_response);
        }
        let snapshot = RequestSnapshot::from_req(&req);
        let mut result = self.inner.call(req).await.map(IntoResponse::into_response);
        // Innermost-first — the entry closest to the handler sees the error
        // before the outer ones, as the nested endpoints did.
        for entry in self.chain.iter().rev() {
            result = match result {
                Ok(resp) => Ok(resp),
                Err(err) => {
                    // `as_ref()`: dispatch on the erased filter — the
                    // `Filter for Arc<T>` blanket would nest a second boxed
                    // future around the call.
                    let mut resp = entry.layer.as_ref().filter(&snapshot, err).await;
                    // Same tag as `FilterEndpoint`: the handler failed, the
                    // mapped status must not bless its writes.
                    resp.extensions_mut().insert(nest_rs_core::MappedError);
                    Ok(resp)
                }
            };
        }
        result
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

    struct BodyTagFilter(&'static str);

    impl Layer for BodyTagFilter {}

    #[async_trait]
    impl Filter for BodyTagFilter {
        async fn filter(&self, _req: &RequestSnapshot, _error: poem::Error) -> Response {
            Response::builder()
                .status(StatusCode::IM_A_TEAPOT)
                .body(self.0)
        }
    }

    fn resolved(
        layer: std::sync::Arc<dyn Filter>,
        name: &'static str,
    ) -> ResolvedLayer<dyn Filter> {
        ResolvedLayer {
            type_id: std::any::TypeId::of::<TeapotFilter>(),
            name,
            source: nest_rs_core::layer_chain::LayerSite::Method,
            layer,
        }
    }

    #[tokio::test]
    async fn a_chain_passes_success_through_untouched() {
        let ep = FilterChain::new(
            ok_handler,
            vec![resolved(std::sync::Arc::new(TeapotFilter), "teapot")],
        );
        let resp = ep.call(Request::default()).await.expect("success flows");
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.extensions()
                .get::<nest_rs_core::MappedError>()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_chain_maps_an_error_at_the_innermost_entry() {
        let failing = make(|_req: Request| async {
            Err::<Response, _>(poem::Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
        });
        // Outermost-first order: "outer" is listed first, "inner" last — the
        // nested endpoints gave the error to the innermost filter, and so
        // must the chain.
        let ep = FilterChain::new(
            failing,
            vec![
                resolved(std::sync::Arc::new(BodyTagFilter("outer")), "outer"),
                resolved(std::sync::Arc::new(BodyTagFilter("inner")), "inner"),
            ],
        );
        let resp = ep.call(Request::default()).await.expect("error is mapped");
        assert_eq!(resp.status(), StatusCode::IM_A_TEAPOT);
        assert!(
            resp.extensions()
                .get::<nest_rs_core::MappedError>()
                .is_some(),
            "a mapped error response must carry the rollback tag",
        );
        let body = resp.into_body().into_string().await.expect("body");
        assert_eq!(body, "inner", "the innermost (last-listed) filter maps");
    }
}
