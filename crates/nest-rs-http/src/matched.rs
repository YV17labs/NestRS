//! Which route the router matched, carried out of it for a request dropped
//! before it answered: poem attaches the template to the response alone.
//!
//! poem's router offers no hook a nested route runs, so a cut request on an
//! endpoint handed to `HttpTransport::mount` is named for its mount path.

use std::sync::{Arc, Mutex, PoisonError};

use poem::{Endpoint, IntoEndpoint, PathPattern, Request};

/// The template poem's router matched one request on, once an endpoint the
/// router reached has noted it. The innermost note wins, as on a response.
#[derive(Clone, Default)]
pub(crate) struct MatchedRoute(Arc<Mutex<Option<Arc<str>>>>);

impl MatchedRoute {
    /// The template noted for this request, if an endpoint the router reached
    /// noted one.
    pub(crate) fn route(&self) -> Option<Arc<str>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Note on `req`'s slot the template the router matched it on; a request the
/// edge did not open carries no slot.
pub(crate) fn note(req: &Request) {
    if let (Some(slot), Some(PathPattern(pattern))) =
        (req.data::<MatchedRoute>(), req.data::<PathPattern>())
    {
        *slot.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(pattern));
    }
}

/// `endpoint`, noting the route poem's router matched as each request reaches
/// it — what a framework surface wraps the endpoint it mounts in.
pub fn matched<E: IntoEndpoint>(endpoint: E) -> Matched<E::Endpoint> {
    Matched(endpoint.into_endpoint())
}

/// An endpoint that notes the route the router matched before it runs. Built
/// by [`matched`].
pub struct Matched<E>(E);

impl<E: Endpoint> Endpoint for Matched<E> {
    type Output = E::Output;

    async fn call(&self, req: Request) -> poem::Result<Self::Output> {
        note(&req);
        self.0.call(req).await
    }
}
