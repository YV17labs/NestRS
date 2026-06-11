use std::sync::Arc;

use nest_rs_core::Container;
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

/// Kept here so `#[routes]` emits `::nest_rs_http::schema_of::<T>` and never
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
    /// A controller- or method-level `#[use_guards]` covers this route. Read at
    /// boot by the fail-secure posture check. A global guard pool covers every
    /// route regardless, so the check only consults this when no pool is active.
    pub scoped_guarded: bool,
    /// `#[public]` — an explicit, intentional public surface. Suppresses the
    /// posture warning (the access decision was made deliberately).
    pub public: bool,
}

impl HttpRouteMeta {
    /// The route's access decision is **implicit**: no global guard pool covers
    /// it, it binds no controller/method guard, and it is not marked
    /// `#[public]`. The HTTP transport warns on these at boot so the developer
    /// guards the route or declares it public on purpose — never by omission.
    pub fn access_is_implicit(&self, global_guards: bool) -> bool {
        !global_guards && !self.scoped_guarded && !self.public
    }
}

type MountFn = dyn Fn(&Container, Route) -> Route + Send + Sync;

/// Discovery metadata attached to every `#[controller]` + `#[routes]` type.
/// [`crate::HttpTransport`] iterates these at boot via
/// [`nest_rs_core::DiscoveryService::meta`]; apps can read the same metadata
/// to drive secondary concerns (OpenAPI rendering, route listings).
pub struct HttpControllerMeta {
    /// The controller struct name (`UsersController`). Links a mounted route
    /// back to its source type — surfaced as a field in the boot route log and
    /// the default OpenAPI tag.
    pub controller: &'static str,
    pub path: &'static str,
    pub version: Option<&'static str>,
    pub routes: Vec<HttpRouteMeta>,
    mount: Arc<MountFn>,
}

impl HttpControllerMeta {
    pub fn new<F>(
        controller: &'static str,
        path: &'static str,
        version: Option<&'static str>,
        routes: Vec<HttpRouteMeta>,
        mount: F,
    ) -> Self
    where
        F: Fn(&Container, Route) -> Route + Send + Sync + 'static,
    {
        Self {
            controller,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn http_verb_as_str_renders_each_method_name() {
        assert_eq!(HttpVerb::Get.as_str(), "GET");
        assert_eq!(HttpVerb::Post.as_str(), "POST");
        assert_eq!(HttpVerb::Put.as_str(), "PUT");
        assert_eq!(HttpVerb::Delete.as_str(), "DELETE");
        assert_eq!(HttpVerb::Patch.as_str(), "PATCH");
    }

    #[test]
    fn http_verb_is_value_type_for_equality_and_clone() {
        // The derives are part of the public surface (`#[routes]` clones the
        // verb into discovery metadata); pin them.
        let a = HttpVerb::Get;
        let b = a;
        assert_eq!(a, b);
        assert_eq!(format!("{a:?}"), "Get");
    }

    #[test]
    fn schema_of_records_a_subschema_for_the_payload_type() {
        let mut generator = schemars::SchemaGenerator::default();
        let schema = schema_of::<String>(&mut generator);
        // The subschema is a JSON-schema object whose serialization round-trips.
        let value: serde_json::Value = serde_json::to_value(&schema).expect("schema serializes");
        assert!(value.is_object(), "schema serializes to a JSON object");
    }

    #[test]
    fn effective_prefix_returns_path_unchanged_without_a_version() {
        let meta =
            HttpControllerMeta::new("UsersController", "/users", None, Vec::new(), |_c, r| r);
        assert_eq!(meta.effective_prefix(), "/users");
    }

    #[test]
    fn effective_prefix_prepends_the_uri_version_when_present() {
        // `version_path` joins `/v<v>` ahead of the controller path — the
        // single place URI versioning lives, so this is the contract.
        let meta =
            HttpControllerMeta::new("UsersController", "/users", Some("1"), Vec::new(), |_c, r| r);
        assert_eq!(meta.effective_prefix(), "/v1/users");
    }

    #[test]
    fn new_stores_the_path_version_and_routes_verbatim() {
        let routes = vec![HttpRouteMeta {
            verb: HttpVerb::Get,
            path: "/:id",
            handler: "show",
            summary: Some("Fetch one"),
            description: None,
            tags: &["Users"],
            request_body: None,
            response: None,
            scoped_guarded: false,
            public: false,
        }];
        let meta =
            HttpControllerMeta::new("UsersController", "/users", Some("2"), routes, |_c, r| r);
        assert_eq!(meta.path, "/users");
        assert_eq!(meta.version, Some("2"));
        assert_eq!(meta.routes.len(), 1);
        assert_eq!(meta.routes[0].handler, "show");
        assert_eq!(meta.routes[0].tags, &["Users"]);
    }

    #[test]
    fn mount_invokes_the_closure_with_the_container_and_route() {
        // The mount closure is the seam `#[routes]` emits; assert it's called
        // exactly once per `mount` invocation and receives the same container.
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let meta = HttpControllerMeta::new("HealthController", "/health", None, Vec::new(), |_c, r| {
            CALLS.fetch_add(1, Ordering::SeqCst);
            r
        });
        let container = Container::builder().build();
        let route = Route::new();

        let _routed = meta.mount(&container, route);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        let _ = meta.mount(&container, Route::new());
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }

    fn route(scoped_guarded: bool, public: bool) -> HttpRouteMeta {
        HttpRouteMeta {
            verb: HttpVerb::Post,
            path: "/",
            handler: "create",
            summary: None,
            description: None,
            tags: &[],
            request_body: None,
            response: None,
            scoped_guarded,
            public,
        }
    }

    #[test]
    fn access_is_implicit_only_when_uncovered_and_no_global_pool() {
        // The one case the posture check warns on: no global pool, no scoped
        // guard, not public.
        assert!(route(false, false).access_is_implicit(false));
    }

    #[test]
    fn a_global_pool_covers_every_route() {
        // With the pool active the route is shaped regardless of its own decls.
        assert!(!route(false, false).access_is_implicit(true));
    }

    #[test]
    fn a_scoped_guard_or_public_marker_makes_the_decision_explicit() {
        // No global pool, but the route owns its decision either way.
        assert!(!route(true, false).access_is_implicit(false));
        assert!(!route(false, true).access_is_implicit(false));
        assert!(!route(true, true).access_is_implicit(false));
    }
}
