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

/// Maps errors returned by the inner endpoint to a response. Runs only on the
/// error path; successful responses pass through.
///
/// `Filter` extends [`Layer`] so the same impl can be declared at any scope
/// and the Layer System dedups by [`TypeId`](std::any::TypeId).
#[async_trait]
pub trait Filter: Layer {
    async fn filter(&self, req: &RequestSnapshot, error: poem::Error) -> Response;
}

#[async_trait]
impl<T: Filter + ?Sized> Filter for Arc<T> {
    async fn filter(&self, req: &RequestSnapshot, error: poem::Error) -> Response {
        (**self).filter(req, error).await
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
            Err(err) => Ok(self.filter.filter(&snapshot, err).await),
        }
    }
}
