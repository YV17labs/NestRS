use std::sync::Arc;

use nest_rs_core::Container;
use poem::Route;

/// Mounts a controller's routes onto a parent [`Route`]; implemented by `#[routes]`.
pub trait Controller: 'static {
    /// Attach this controller's routes (under its `PATH`) onto `route`,
    /// resolving handler dependencies from `container`.
    fn mount(container: &Container, route: Route) -> Route;
}

/// The HTTP method a route answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpVerb {
    /// `GET`.
    Get,
    /// `POST`.
    Post,
    /// `PUT`.
    Put,
    /// `DELETE`.
    Delete,
    /// `PATCH`.
    Patch,
}

impl HttpVerb {
    /// The uppercase method token (`"GET"`, `"POST"`, …).
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

/// The response header `#[crud]`'s paginated list names the next page's cursor
/// in, absent on the last page.
pub const NEXT_CURSOR_HEADER: &str = "x-next-cursor";

/// Builds the schema for a request body or response, recording named component
/// schemas in the shared generator.
pub type SchemaFn = fn(&mut schemars::SchemaGenerator) -> schemars::Schema;

/// The schema of `T`, as `#[routes]` emits it into a [`SchemaFn`].
pub fn schema_of<T: schemars::JsonSchema>(
    generator: &mut schemars::SchemaGenerator,
) -> schemars::Schema {
    generator.subschema_for::<T>()
}

/// The request body a route accepts: how it arrives on the wire, and the
/// schema of its content when the framework can name the type.
#[derive(Clone, Copy)]
pub enum RequestBodyMeta {
    /// A `Json<T>` extractor — `application/json`, carrying `T`'s schema.
    Json(SchemaFn),
    /// A `multipart/form-data` body: `Some` when `#[api(multipart = T)]` names
    /// the form's type, `None` for a bare [`poem::web::Multipart`] parameter.
    Multipart(Option<SchemaFn>),
    /// A `Form<T>` extractor — `application/x-www-form-urlencoded`, carrying
    /// `T`'s schema.
    Form(SchemaFn),
}

impl RequestBodyMeta {
    /// The `content` key an OpenAPI document files this body under.
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Json(_) => "application/json",
            Self::Multipart(_) => "multipart/form-data",
            Self::Form(_) => "application/x-www-form-urlencoded",
        }
    }

    /// The schema builder for the body's content, when the body has a named type.
    pub fn schema(self) -> Option<SchemaFn> {
        match self {
            Self::Json(schema) => Some(schema),
            Self::Multipart(schema) => schema,
            Self::Form(schema) => Some(schema),
        }
    }
}

/// Declarative description of a handler in a controller — verb/path/name plus
/// the OpenAPI facets `#[routes]` extracts, so a doc generator (nest-rs-openapi)
/// builds a spec from discovery alone.
///
/// Built only by the `#[routes]` macro: its fields are an internal ABI that
/// versions in lockstep with it, not a hand-written surface.
#[derive(Clone)]
pub struct HttpRouteMeta {
    /// The method this route answers.
    pub verb: HttpVerb,
    /// The route path, relative to the controller prefix.
    pub path: &'static str,
    /// The handler method's name — the `handler` field in the boot route log.
    pub handler: &'static str,
    /// `#[version("2")]` on the method — the versions this route serves, out of
    /// the ones its controller declares; empty serves every one of them.
    pub versions: &'static [&'static str],
    /// `#[api(summary = …)]` one-liner for the OpenAPI operation, if given.
    pub summary: Option<&'static str>,
    /// `#[api(description = …)]` long text for the OpenAPI operation, if given.
    pub description: Option<&'static str>,
    /// `#[api(tags(...))]`, else a single-element slice holding the controller
    /// struct name — so routes group by controller in the docs by default.
    pub tags: &'static [&'static str],
    /// The request body this route accepts, or `None` when it takes none.
    pub request_body: Option<RequestBodyMeta>,
    /// Schema builder for the response payload — inferred from a `Json<T>`
    /// return, or declared with `#[api(response = T)]` when the handler builds
    /// its own [`Response`](poem::Response).
    pub response: Option<SchemaFn>,
    /// The media type of the success response body, when it is **not**
    /// `application/json` — `#[api(response_content_type = "audio/mpeg")]`, or
    /// `text/event-stream` inferred from an `-> SSE` return.
    pub response_content_type: Option<&'static str>,
    /// An ability shaper (`Authorize<_, _>`) masks this route's response, so a
    /// caller may receive a **subset** of [`response`](Self::response)'s
    /// properties — whichever ones its ability grants.
    pub masked: bool,
    /// Schema builders for the handler's `Path<T>` extractor components, in
    /// path order (a `Path<(A, B)>` tuple yields one per element); empty when
    /// the handler binds its id another way (`Bind<_, _>`).
    pub path_params: &'static [SchemaFn],
    /// Schema builders for the handler's `Query<T>` extractor payloads, each
    /// expanded into one OpenAPI `query` parameter per property.
    pub query_params: &'static [SchemaFn],
    /// Schema builders for the handler's [`Header<T>`](crate::Header) extractor
    /// payloads, expanded exactly like [`query_params`](Self::query_params) but
    /// into `in: header` parameters. A property absent from the schema's
    /// `required` is an optional header.
    pub header_params: &'static [SchemaFn],
    /// The operation is a write that can surface a `409 Conflict` — set by
    /// `#[crud]`'s create/update/delete ops.
    pub may_conflict: bool,
    /// A `ThrottlerGuard` rate-limits this route, so it can answer `429` with a
    /// `Retry-After` header. Detected by the guard's type name, so a handler
    /// throttling by other means leaves it `false`.
    pub throttled: bool,
    /// The route's success response carries a `Location` header that the
    /// framework emits (`#[crud]`'s create, `#[redirect]`); a hand-written
    /// handler setting it itself leaves this `false`.
    pub sets_location: bool,
    /// `#[crud]`'s paginated list: the success response carries
    /// [`NEXT_CURSOR_HEADER`] whenever another page follows.
    pub sets_next_cursor: bool,
    /// Each `#[response_header(name, value)]` on the handler, as written: the
    /// success response always carries these.
    pub response_headers: &'static [(&'static str, &'static str)],
    /// The effective **success** HTTP status this route emits — `200` unless a
    /// `#[http_code(N)]` or `#[redirect(_, code)]` overrides it.
    pub success_status: u16,
    /// A controller- or method-level `#[use_guards]` covers this route; read at
    /// boot by the fail-secure posture check.
    pub scoped_guarded: bool,
    /// `#[public]` — an explicit, intentional public surface. Suppresses the
    /// posture warning.
    pub public: bool,
}

impl HttpRouteMeta {
    /// The route's access decision is **implicit**: no global guard pool covers
    /// it, it binds no controller/method guard, and it is not marked
    /// `#[public]`. The HTTP transport warns on these at boot.
    pub fn access_is_implicit(&self, global_guards: bool) -> bool {
        !global_guards && !self.scoped_guarded && !self.public
    }
}

type MountFn = dyn Fn(&Container, Route) -> Route + Send + Sync;

/// Discovery metadata attached to every `#[controller]` + `#[routes]` type,
/// iterated by [`crate::HttpTransport`] at boot via
/// [`nest_rs_core::Discovery::meta`].
pub struct HttpControllerMeta {
    /// The controller struct name (`UsersController`).
    pub controller: &'static str,
    /// The controller's name as an identifier fragment (`PostsController` →
    /// `posts`), the stem of each OpenAPI `operationId`; a name that is *only*
    /// the `Controller` suffix keeps it.
    ///
    /// Computed by `#[routes]` through `nest_rs_codegen::snake_case`: a runtime
    /// crate reaching `codegen` would pull `syn` into every app.
    pub token: &'static str,
    /// The controller's shared path prefix (before URI versioning).
    pub path: &'static str,
    /// `#[controller(version = …)]` — every version this controller serves, in
    /// declaration order. Empty means unversioned.
    pub versions: &'static [&'static str],
    /// Metadata for each route this controller declares.
    pub routes: Vec<HttpRouteMeta>,
    mount: Arc<MountFn>,
}

impl HttpControllerMeta {
    /// Assemble the discovery metadata for one controller; `mount` closes over
    /// the handler wiring.
    pub fn new<F>(
        controller: &'static str,
        token: &'static str,
        path: &'static str,
        versions: &'static [&'static str],
        routes: Vec<HttpRouteMeta>,
        mount: F,
    ) -> Self
    where
        F: Fn(&Container, Route) -> Route + Send + Sync + 'static,
    {
        Self {
            controller,
            token,
            path,
            versions,
            routes,
            mount: Arc::new(mount),
        }
    }

    /// The versions this controller mounts under, as
    /// [`version_path`](crate::version_path) wants them: one `None` when it is
    /// unversioned, otherwise one `Some(v)` per declared version.
    pub fn mounted_versions(&self) -> impl Iterator<Item = Option<&'static str>> + '_ {
        // Yielding nothing for an unversioned controller would unmount it.
        let unversioned = self.versions.is_empty().then_some(None);
        unversioned
            .into_iter()
            .chain(self.versions.iter().map(|v| Some(*v)))
    }

    /// Mount prefix for one of [`mounted_versions`](Self::mounted_versions)
    /// (`/v1/users`, or `/users` for `None`).
    pub fn effective_prefix(&self, version: Option<&str>) -> String {
        crate::version_path(version, self.path)
    }

    /// Whether `route` is served under `version`. An undecorated route serves
    /// every version its controller declares; `#[version("2")]` narrows it.
    pub fn serves(route: &HttpRouteMeta, version: Option<&str>) -> bool {
        match (route.versions, version) {
            ([], _) => true,
            (_, None) => true,
            (declared, Some(v)) => declared.contains(&v),
        }
    }

    /// Mount this controller's routes onto `route`, resolving handler
    /// dependencies from `container`.
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
        let a = HttpVerb::Get;
        let b = a;
        assert_eq!(a, b);
        assert_eq!(format!("{a:?}"), "Get");
    }

    #[test]
    fn schema_of_records_a_subschema_for_the_payload_type() {
        let mut generator = schemars::SchemaGenerator::default();
        let schema = schema_of::<String>(&mut generator);
        let value: serde_json::Value = serde_json::to_value(&schema).expect("schema serializes");
        assert!(value.is_object(), "schema serializes to a JSON object");
    }

    #[test]
    fn request_body_meta_pairs_each_media_type_with_its_own_schema() {
        fn schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
            generator.subschema_for::<String>()
        }
        let json = RequestBodyMeta::Json(schema);
        assert_eq!(json.media_type(), "application/json");
        assert!(json.schema().is_some());

        let typed = RequestBodyMeta::Multipart(Some(schema));
        assert_eq!(typed.media_type(), "multipart/form-data");
        assert!(typed.schema().is_some());

        let untyped = RequestBodyMeta::Multipart(None);
        assert_eq!(untyped.media_type(), "multipart/form-data");
        assert!(untyped.schema().is_none());
    }

    #[test]
    fn an_unversioned_controller_still_mounts_at_one_address() {
        let meta = HttpControllerMeta::new(
            "UsersController",
            "users",
            "/users",
            &[],
            Vec::new(),
            |_c, r| r,
        );
        let mounted: Vec<_> = meta.mounted_versions().collect();
        assert_eq!(mounted, vec![None]);
        assert_eq!(meta.effective_prefix(None), "/users");
    }

    #[test]
    fn each_declared_version_is_its_own_mount_prefix() {
        let meta = HttpControllerMeta::new(
            "UsersController",
            "users",
            "/users",
            &["1", "2"],
            Vec::new(),
            |_c, r| r,
        );
        let prefixes: Vec<_> = meta
            .mounted_versions()
            .map(|v| meta.effective_prefix(v))
            .collect();
        assert_eq!(prefixes, ["/v1/users", "/v2/users"]);
    }

    #[test]
    fn a_route_serves_every_controller_version_until_it_narrows_itself() {
        let mut route = route_meta();
        assert!(
            HttpControllerMeta::serves(&route, Some("1")),
            "an undecorated route serves every version its controller declares",
        );
        route.versions = &["2"];
        assert!(HttpControllerMeta::serves(&route, Some("2")));
        assert!(
            !HttpControllerMeta::serves(&route, Some("1")),
            "`#[version(\"2\")]` narrows the route out of v1",
        );
        assert!(
            HttpControllerMeta::serves(&route, None),
            "an unversioned mount has one address, so a narrowed route still serves it",
        );
    }

    #[test]
    fn versions_declare_accepts_a_subset_and_refuses_a_stranger() {
        assert!(crate::versions_declare(&["1", "2"], &["2"]));
        assert!(crate::versions_declare(&["1", "2"], &["1", "2"]));
        assert!(crate::versions_declare(&["1"], &[]));
        assert!(!crate::versions_declare(&["1", "2"], &["3"]));
        assert!(!crate::versions_declare(&[], &["1"]));
        // Length-first comparison must not report a prefix as equal.
        assert!(!crate::versions_declare(&["1"], &["11"]));
    }

    fn route_meta() -> HttpRouteMeta {
        HttpRouteMeta {
            verb: HttpVerb::Get,
            path: "/:id",
            handler: "show",
            summary: Some("Fetch one"),
            description: None,
            tags: &["Users"],
            request_body: None,
            response: None,
            response_content_type: None,
            masked: false,
            path_params: &[],
            query_params: &[],
            header_params: &[],
            may_conflict: false,
            throttled: false,
            sets_location: false,
            sets_next_cursor: false,
            response_headers: &[],
            success_status: 200,
            scoped_guarded: false,
            public: false,
            versions: &[],
        }
    }

    #[test]
    fn new_stores_the_path_versions_and_routes_verbatim() {
        let meta = HttpControllerMeta::new(
            "UsersController",
            "users",
            "/users",
            &["2"],
            vec![route_meta()],
            |_c, r| r,
        );
        assert_eq!(meta.path, "/users");
        assert_eq!(meta.versions, &["2"]);
        assert_eq!(meta.routes.len(), 1);
        assert_eq!(meta.routes[0].handler, "show");
        assert_eq!(meta.routes[0].tags, &["Users"]);
    }

    #[test]
    fn mount_invokes_the_closure_with_the_container_and_route() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let meta = HttpControllerMeta::new(
            "HealthController",
            "health",
            "/health",
            &[],
            Vec::new(),
            |_c, r| {
                CALLS.fetch_add(1, Ordering::SeqCst);
                r
            },
        );
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
            response_content_type: None,
            masked: false,
            path_params: &[],
            query_params: &[],
            header_params: &[],
            may_conflict: false,
            throttled: false,
            sets_location: false,
            sets_next_cursor: false,
            response_headers: &[],
            success_status: 200,
            scoped_guarded,
            public,
            versions: &[],
        }
    }

    #[test]
    fn access_is_implicit_only_when_uncovered_and_no_global_pool() {
        assert!(route(false, false).access_is_implicit(false));
    }

    #[test]
    fn a_global_pool_covers_every_route() {
        assert!(!route(false, false).access_is_implicit(true));
    }

    #[test]
    fn a_scoped_guard_or_public_marker_makes_the_decision_explicit() {
        assert!(!route(true, false).access_is_implicit(false));
        assert!(!route(false, true).access_is_implicit(false));
        assert!(!route(true, true).access_is_implicit(false));
    }
}
