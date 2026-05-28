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

impl HttpVerb {
    /// Upper-case method name, e.g. for the boot route log.
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

/// Builds the schema for a `Json<T>` request body or response, recording any
/// named component schemas in the shared generator and returning either an
/// inline schema or a `$ref` into `components/schemas`. `#[routes]` emits one
/// (`schema_of::<T>`) per JSON payload it finds; a handler whose body/return is
/// not `Json<…>` (a raw `Response`, `String`, `StatusCode`, …) carries `None`
/// and imposes no `JsonSchema` bound.
pub type SchemaFn = fn(&mut schemars::SchemaGenerator) -> schemars::Schema;

/// The [`SchemaFn`] the `#[routes]` macro instantiates for a payload type `T`.
/// Kept here so the macro emits `::nestrs_http::schema_of::<T>` and never names
/// `schemars`' generator API itself.
pub fn schema_of<T: schemars::JsonSchema>(
    generator: &mut schemars::SchemaGenerator,
) -> schemars::Schema {
    generator.subschema_for::<T>()
}

/// Declarative description of a single handler inside a controller. Beyond the
/// verb/path/handler the transport needs to mount it, it carries the optional
/// OpenAPI facets the `#[routes]` macro extracts — `#[api(...)]` metadata and
/// the request/response payload schemas — so a documentation generator
/// (nestrs-openapi) can build a spec from discovery alone. Fn-pointer fields
/// are why this type is not `Debug`.
#[derive(Clone)]
pub struct HttpRouteMeta {
    pub verb: HttpVerb,
    pub path: &'static str,
    pub handler: &'static str,
    /// `#[api(summary = "...")]`, else `None`.
    pub summary: Option<&'static str>,
    /// `#[api(description = "...")]`, else `None`.
    pub description: Option<&'static str>,
    /// `#[api(tags(...))]`, else a single-element slice holding the controller
    /// struct name — so routes group by controller in the docs by default.
    pub tags: &'static [&'static str],
    /// Schema of the JSON request body, when the handler takes `Json<T>` /
    /// `Valid<Json<T>>` / `Piped<_, Json<T>>`.
    pub request_body: Option<SchemaFn>,
    /// Schema of the JSON response, when the handler returns `Json<T>`
    /// (optionally wrapped in `Result<…>`).
    pub response: Option<SchemaFn>,
}

type MountFn = dyn Fn(&Container, Route) -> Route + Send + Sync;

/// Discovery metadata attached to every `#[controller]` + `#[routes]` type
/// by the macros. The [`crate::HttpTransport`] iterates these via
/// [`nestrs_core::DiscoveryService::meta`] at boot to assemble the root
/// route. Apps can read the same metadata to drive secondary concerns
/// (OpenAPI rendering, route listings) without touching the transport.
pub struct HttpControllerMeta {
    pub path: &'static str,
    /// URI API version from `#[controller(version = "1")]`: `Some("1")` mounts
    /// the controller under `/v1`, `None` mounts it at `path` unversioned.
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

    /// The controller's mount prefix with URI versioning applied — `/v1/users`
    /// when versioned, `/users` otherwise. Readers that compose full route paths
    /// (the boot route log, the OpenAPI document) join each route onto this so
    /// they match what [`mount`](Self::mount) actually serves.
    pub fn effective_prefix(&self) -> String {
        crate::version_path(self.version, self.path)
    }

    /// Mount this controller's routes onto `route`, using `container` to
    /// resolve the controller's dependencies. Called by `HttpTransport`.
    pub fn mount(&self, container: &Container, route: Route) -> Route {
        (self.mount)(container, route)
    }
}
