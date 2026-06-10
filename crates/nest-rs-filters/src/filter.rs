//! The [`Filter`] trait — extends [`Layer`] for the Layer System.

use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_graphql::async_graphql::{Context as GraphqlContext, Error as GraphqlError};
use nest_rs_ws::WsClient;
use poem::http::{HeaderMap, Method, Uri};
use poem::{Endpoint, IntoResponse, Request, Response, Result};

/// Read-only view of the request handed to a [`Filter`]. The original
/// `poem::Request` has been consumed by the inner endpoint by the time the
/// filter runs (and is not `Clone`), so the routing-relevant bits are
/// captured up front.
#[derive(Debug, Clone)]
pub struct RequestSnapshot {
    pub method: Method,
    pub uri: Uri,
    pub headers: HeaderMap,
}

impl RequestSnapshot {
    pub fn from_req(req: &Request) -> Self {
        Self {
            method: req.method().clone(),
            uri: req.uri().clone(),
            headers: req.headers().clone(),
        }
    }
}

/// Maps errors returned by the inner handler to a response (HTTP) or a
/// reshaped error frame (GraphQL / WS). Runs only on the error path;
/// successful results pass through unchanged.
///
/// One impl, three transports — override only the method(s) the filter
/// targets. `Filter` extends [`Layer`] so global + per-scope declarations
/// dedup by [`TypeId`](std::any::TypeId).
#[async_trait]
pub trait Filter: Layer {
    /// HTTP entry — required, no default: a filter that targets HTTP
    /// without implementing this would silently let errors through.
    async fn filter(&self, req: &RequestSnapshot, error: poem::Error) -> Response;

    /// GraphQL per-resolver entry — a reserved seam, **not wired** today
    /// (no macro or dispatcher calls it; a global filter still covers the
    /// `/graphql` POST as a whole via its HTTP entry). Default returns
    /// `error` unchanged (no-op).
    async fn filter_graphql<'a>(
        &self,
        _ctx: &GraphqlContext<'a>,
        error: GraphqlError,
    ) -> GraphqlError {
        error
    }

    /// WS per-message entry — a reserved seam, **not wired** today (no
    /// macro or dispatcher calls it). Default returns `error` unchanged
    /// (no-op).
    async fn filter_ws(&self, _client: &WsClient, _event: &str, error: String) -> String {
        error
    }
}

#[async_trait]
impl<T: Filter + ?Sized> Filter for Arc<T> {
    async fn filter(&self, req: &RequestSnapshot, error: poem::Error) -> Response {
        (**self).filter(req, error).await
    }

    async fn filter_graphql<'a>(
        &self,
        ctx: &GraphqlContext<'a>,
        error: GraphqlError,
    ) -> GraphqlError {
        (**self).filter_graphql(ctx, error).await
    }

    async fn filter_ws(&self, client: &WsClient, event: &str, error: String) -> String {
        (**self).filter_ws(client, event, error).await
    }
}

pub struct FilterEndpoint<E, F> {
    inner: E,
    filter: F,
}

impl<E, F> FilterEndpoint<E, F> {
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
