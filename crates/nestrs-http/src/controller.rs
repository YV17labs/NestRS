use std::sync::Arc;

use nestrs_core::Container;
use poem::Route;

/// Implemented automatically by the `#[routes]` macro. Each controller
/// exposes a single entry point that mounts its routes (already prefixed
/// with the controller's `PATH`) onto a parent [`Route`].
pub trait Controller: 'static {
    fn mount(container: &Container, route: Route) -> Route;
}

/// HTTP verbs recognised by the `#[routes]` macro. The metadata layer
/// exposes the verb declaratively so non-mounting scanners (an OpenAPI
/// generator, a docs page, an introspection endpoint) can read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpVerb {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

/// Declarative description of a single handler inside a controller.
#[derive(Clone, Debug)]
pub struct HttpRouteMeta {
    pub verb: HttpVerb,
    pub path: &'static str,
    pub handler: &'static str,
}

type MountFn = dyn Fn(&Container, Route) -> Route + Send + Sync;

/// Discovery metadata attached to every `#[controller]` + `#[routes]` type
/// by the macros. The [`crate::HttpTransport`] iterates these via
/// [`nestrs_core::DiscoveryService::meta`] at boot to assemble the root
/// route. Apps can read the same metadata to drive secondary concerns
/// (OpenAPI rendering, route listings) without touching the transport.
pub struct HttpControllerMeta {
    pub path: &'static str,
    pub routes: Vec<HttpRouteMeta>,
    mount: Arc<MountFn>,
}

impl HttpControllerMeta {
    pub fn new(path: &'static str, routes: Vec<HttpRouteMeta>, mount: Arc<MountFn>) -> Self {
        Self {
            path,
            routes,
            mount,
        }
    }

    /// Mount this controller's routes onto `route`, using `container` to
    /// resolve the controller's dependencies. Called by `HttpTransport`.
    pub fn mount(&self, container: &Container, route: Route) -> Route {
        (self.mount)(container, route)
    }
}
