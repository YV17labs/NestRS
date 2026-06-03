use std::sync::Arc;

use nestrs_core::Container;
use poem::Route;

/// Implemented automatically by the `#[routes]` macro. Each controller
/// mounts its routes (prefixed with the controller's `PATH`) onto a parent
/// [`Route`].
pub trait Controller: 'static {
    fn mount(container: &Container, route: Route) -> Route;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpVerb {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

impl HttpVerb {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Patch => "PATCH",
        }
    }
}

/// Builds the schema for a `Json<T>` request body or response, recording
/// named component schemas in the shared generator. `#[routes]` emits one per
/// JSON payload it finds; a non-`Json<…>` body/return carries `None` and
/// imposes no `JsonSchema` bound.
pub type SchemaFn = fn(&mut schemars::SchemaGenerator) -> schemars::Schema;

/// Kept here so `#[routes]` emits `::nestrs_http::schema_of::<T>` and never
/// names `schemars`' generator API itself.
pub fn schema_of<T: schemars::JsonSchema>(
    generator: &mut schemars::SchemaGenerator,
) -> schemars::Schema {
    generator.subschema_for::<T>()
}

/// Declarative description of a handler in a controller — verb/path/name plus
/// the OpenAPI facets `#[routes]` extracts, so a doc generator (nestrs-openapi)
/// builds a spec from discovery alone.
#[derive(Clone)]
pub struct HttpRouteMeta {
    pub verb: HttpVerb,
    pub path: &'static str,
    pub handler: &'static str,
    pub summary: Option<&'static str>,
    pub description: Option<&'static str>,
    /// `#[api(tags(...))]`, else a single-element slice holding the controller
    /// struct name — so routes group by controller in the docs by default.
    pub tags: &'static [&'static str],
    pub request_body: Option<SchemaFn>,
    pub response: Option<SchemaFn>,
}

type MountFn = dyn Fn(&Container, Route) -> Route + Send + Sync;

/// Discovery metadata attached to every `#[controller]` + `#[routes]` type.
/// [`crate::HttpTransport`] iterates these at boot via
/// [`nestrs_core::DiscoveryService::meta`]; apps can read the same metadata
/// to drive secondary concerns (OpenAPI rendering, route listings).
pub struct HttpControllerMeta {
    pub path: &'static str,
    pub version: Option<&'static str>,
    pub routes: Vec<HttpRouteMeta>,
    mount: Arc<MountFn>,
}

impl HttpControllerMeta {
    pub fn new<F>(
        path: &'static str,
        version: Option<&'static str>,
        routes: Vec<HttpRouteMeta>,
        mount: F,
    ) -> Self
    where
        F: Fn(&Container, Route) -> Route + Send + Sync + 'static,
    {
        Self {
            path,
            version,
            routes,
            mount: Arc::new(mount),
        }
    }

    /// Mount prefix with URI versioning applied (`/v1/users` when versioned).
    /// Readers composing full route paths (boot log, OpenAPI doc) join each
    /// route onto this so they match what [`mount`](Self::mount) serves.
    pub fn effective_prefix(&self) -> String {
        crate::version_path(self.version, self.path)
    }

    pub fn mount(&self, container: &Container, route: Route) -> Route {
        (self.mount)(container, route)
    }
}
