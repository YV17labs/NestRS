//! Which route the router matched, carried out of it for the one request whose
//! response never brings it back: a request dropped before it answered.
//!
//! The edge names a request's span — `http.route`, and the `otel.name` built
//! from it — off the response, because poem's router attaches the template it
//! matched to whatever comes back. A request cut by the shutdown window, or by a
//! client that reset its connection, brings nothing back, so its span exported
//! under the literal `http.request` with no route: every cancelled request of a
//! deployment grouped as one, though the router had matched each of them.
//!
//! So the edge hands every request a [`MatchedRoute`], and every endpoint the
//! framework mounts notes the template in it as it starts — the controllers'
//! method table, each self-mount's endpoint through [`matched`], the transport's
//! imperative `mount`. It is read only for a request dropped unanswered: an
//! answered one is named off its response as before, so an endpoint that never
//! notes costs a cut request its route, and nothing else.
//!
//! **Below a mount, the developer's routing is out of reach.** poem's router
//! offers no hook a nested route runs, so an endpoint handed to
//! `HttpTransport::mount` that routes further inside itself has a cut request
//! named for its mount path — the template the framework saw it match — where
//! an answered one carries the nested template.

use std::sync::{Arc, Mutex, PoisonError};

use poem::{Endpoint, IntoEndpoint, PathPattern, Request};

/// The template poem's router matched one request on, once an endpoint the
/// router reached has noted it.
///
/// Shared between the edge, which holds it across the request, and the request
/// itself, whose extensions carry it to the endpoint. The innermost note wins,
/// which is the template the router attaches to an answered request's response,
/// so a cut request and an answered one on the same route report the same
/// `http.route`.
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

/// Note on `req`'s slot the template the router matched it on. A request the
/// edge did not open carries no slot — a test driving a bare route — and that is
/// no request to note anything for.
pub(crate) fn note(req: &Request) {
    if let (Some(slot), Some(PathPattern(pattern))) =
        (req.data::<MatchedRoute>(), req.data::<PathPattern>())
    {
        *slot.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(pattern));
    }
}

/// `endpoint`, noting the route poem's router matched as each request reaches
/// it.
///
/// What a framework surface wraps the endpoint it mounts in — at `Route::at` or
/// `Route::nest` — so a request of its own cut before it answers exports its
/// span under its route like an answered one. See the module docs.
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
