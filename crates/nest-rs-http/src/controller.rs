use std::sync::Arc;

use http::Method;
use nest_rs_core::Container;

use crate::deprecation::DeprecationMeta;
use crate::routes::HttpRoutes;

/// Registers a controller's routes on the transport's table; implemented by
/// `#[routes]`.
pub trait Controller: 'static {
    /// Register this controller's routes (under its `PATH`) on `routes`,
    /// resolving handler dependencies from `container`.
    fn mount(container: &Container, routes: &mut HttpRoutes);
}

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
    /// the form's type, `None` for a bare [`Multipart`](crate::Multipart) parameter.
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

/// What `#[routes]` reads off one handler: the method and the template it
/// answers, and each fact an OpenAPI document states about it, one accessor
/// apiece, so a document builds from discovery alone.
///
/// `#[routes]` alone builds one, so a fact a later release reads off a handler
/// is one accessor more; a literal is refused:
///
/// ```compile_fail,E0560
/// # use nest_rs_http::{HttpRouteMeta, Method};
/// let route = HttpRouteMeta {
///     method: Method::GET,
///     path: "/",
///     handler: "list",
/// #   versions: &[],
/// #   summary: None,
/// #   description: None,
/// #   tags: &[],
/// #   request_body: None,
/// #   response: None,
/// #   response_content_type: None,
/// #   error_responses: &[],
/// #   masked: false,
/// #   path_params: &[],
/// #   query_params: &[],
/// #   header_params: &[],
/// #   may_conflict: false,
/// #   throttled: false,
/// #   sets_location: false,
/// #   sets_next_link: false,
/// #   response_headers: &[],
/// #   success_status: 200,
/// #   scoped_guarded: false,
/// #   public: false,
/// #   deprecation: None,
/// };
/// ```
#[derive(Clone)]
pub struct HttpRouteMeta {
    parts: HttpRouteParts,
}

/// The facts `#[routes]` reads off one handler, each the value of the
/// [`HttpRouteMeta`] accessor of the same name.
#[derive(Clone)]
#[expect(
    missing_docs,
    reason = "each field is documented on the `HttpRouteMeta` accessor of its name"
)]
pub struct HttpRouteParts {
    pub method: Method,
    pub path: &'static str,
    pub handler: &'static str,
    pub versions: &'static [&'static str],
    pub summary: Option<&'static str>,
    pub description: Option<&'static str>,
    pub tags: &'static [&'static str],
    pub request_body: Option<RequestBodyMeta>,
    pub response: Option<SchemaFn>,
    pub response_content_type: Option<&'static str>,
    pub error_responses: &'static [(u16, SchemaFn)],
    pub masked: bool,
    pub path_params: &'static [SchemaFn],
    pub query_params: &'static [SchemaFn],
    pub header_params: &'static [SchemaFn],
    pub may_conflict: bool,
    pub throttled: bool,
    pub sets_location: bool,
    pub sets_next_link: bool,
    pub response_headers: &'static [(&'static str, &'static str)],
    pub success_status: u16,
    pub scoped_guarded: bool,
    pub public: bool,
    pub deprecation: Option<DeprecationMeta>,
}

/// The record `#[routes]` builds for one handler out of what it read.
pub fn http_route_meta(parts: HttpRouteParts) -> HttpRouteMeta {
    HttpRouteMeta { parts }
}

impl HttpRouteMeta {
    /// The method this route answers.
    pub fn method(&self) -> &Method {
        &self.parts.method
    }

    /// The route's template (`/{id}`), relative to the controller prefix,
    /// normalized as the router serves it.
    pub fn path(&self) -> &'static str {
        self.parts.path
    }

    /// The handler method's name — the `handler` field in the boot route log.
    pub fn handler(&self) -> &'static str {
        self.parts.handler
    }

    /// `#[version("2")]` on the method — the versions this route serves, out of
    /// the ones its controller declares; empty serves every one of them.
    pub fn versions(&self) -> &'static [&'static str] {
        self.parts.versions
    }

    /// `#[api(summary = …)]` one-liner for the OpenAPI operation, if given.
    pub fn summary(&self) -> Option<&'static str> {
        self.parts.summary
    }

    /// `#[api(description = …)]` long text for the OpenAPI operation, if given.
    pub fn description(&self) -> Option<&'static str> {
        self.parts.description
    }

    /// `#[api(tags(...))]`, else a single-element slice holding the controller's
    /// [`token`](HttpControllerMeta::token) — the stem its `operationId`s carry,
    /// so a controller's routes group under one name by default.
    pub fn tags(&self) -> &'static [&'static str] {
        self.parts.tags
    }

    /// The request body this route accepts, or `None` when it takes none.
    pub fn request_body(&self) -> Option<RequestBodyMeta> {
        self.parts.request_body
    }

    /// Schema builder for the response payload — inferred from a `Json<T>`
    /// return, or declared with `#[api(response = T)]` when the handler builds
    /// its own [`Response`](crate::Response).
    pub fn response(&self) -> Option<SchemaFn> {
        self.parts.response
    }

    /// The media type of the success response body, when it is **not**
    /// `application/json` — `#[api(response_content_type = "audio/mpeg")]`, or
    /// `text/event-stream` inferred from an `#[sse]` route.
    pub fn response_content_type(&self) -> Option<&'static str> {
        self.parts.response_content_type
    }

    /// `#[api(error(503 = T))]` — each error status the handler answers with a
    /// JSON body of its own, and that body's schema.
    pub fn error_responses(&self) -> &'static [(u16, SchemaFn)] {
        self.parts.error_responses
    }

    /// An ability shaper (`Authorize<_, _>`) masks this route's response, so a
    /// caller may receive a **subset** of [`response`](Self::response)'s
    /// properties — whichever ones its ability grants.
    pub fn masked(&self) -> bool {
        self.parts.masked
    }

    /// Schema builders for the handler's `Path<T>` extractor components, in
    /// path order (a `Path<(A, B)>` tuple yields one per element); empty when
    /// the handler binds its id another way (`Bind<_, _>`).
    pub fn path_params(&self) -> &'static [SchemaFn] {
        self.parts.path_params
    }

    /// Schema builders for the handler's `Query<T>` extractor payloads, each
    /// expanded into one OpenAPI `query` parameter per property.
    pub fn query_params(&self) -> &'static [SchemaFn] {
        self.parts.query_params
    }

    /// Schema builders for the handler's [`Header<T>`](crate::Header) extractor
    /// payloads, expanded exactly like [`query_params`](Self::query_params) but
    /// into `in: header` parameters. A property absent from the schema's
    /// `required` is an optional header.
    pub fn header_params(&self) -> &'static [SchemaFn] {
        self.parts.header_params
    }

    /// The operation is a write that can surface a `409 Conflict` — set by
    /// `#[crud]`'s create/update/delete ops.
    pub fn may_conflict(&self) -> bool {
        self.parts.may_conflict
    }

    /// A `ThrottlerGuard` rate-limits this route, so it can answer `429` with a
    /// `Retry-After` header. Detected by the guard's type name, so a handler
    /// throttling by other means reads `false`.
    pub fn throttled(&self) -> bool {
        self.parts.throttled
    }

    /// The route's success response carries a `Location` header that the
    /// framework emits (`#[crud]`'s create, `#[redirect]`); a hand-written
    /// handler setting it itself reads `false`.
    pub fn sets_location(&self) -> bool {
        self.parts.sets_location
    }

    /// `#[crud]`'s paginated list: the success response carries a `Link` with
    /// `rel="next"` whenever another page follows ([`set_next_link`](crate::set_next_link)).
    pub fn sets_next_link(&self) -> bool {
        self.parts.sets_next_link
    }

    /// Each `#[response_header(name, value)]` on the handler, as written: the
    /// success response always carries these.
    pub fn response_headers(&self) -> &'static [(&'static str, &'static str)] {
        self.parts.response_headers
    }

    /// The effective **success** HTTP status this route emits — `200` unless a
    /// `#[http_code(N)]` or `#[redirect(_, code)]` overrides it.
    pub fn success_status(&self) -> u16 {
        self.parts.success_status
    }

    /// A controller- or method-level `#[use_guards]` covers this route; read at
    /// boot by the fail-secure posture check.
    pub fn scoped_guarded(&self) -> bool {
        self.parts.scoped_guarded
    }

    /// `#[public]` — an explicit, intentional public surface. Suppresses the
    /// posture warning.
    pub fn public(&self) -> bool {
        self.parts.public
    }

    /// `#[api(deprecated = "…")]` on the handler: the document marks the
    /// operation, and every answer carries the `Deprecation` header.
    pub fn deprecation(&self) -> Option<DeprecationMeta> {
        self.parts.deprecation
    }

    /// The route's access decision is **implicit**: no global guard pool covers
    /// it, it binds no controller/method guard, and it is not marked
    /// `#[public]`. The HTTP transport warns on these at boot.
    pub fn access_is_implicit(&self, global_guards: bool) -> bool {
        !global_guards && !self.parts.scoped_guarded && !self.parts.public
    }
}

type MountFn = dyn Fn(&Container, &mut HttpRoutes) + Send + Sync;

/// What `#[routes]` attaches for one `#[controller]`: its name, its prefix,
/// its versions and its routes, discovered by [`crate::HttpTransport`] at boot
/// through [`nest_rs_core::Discovery::meta`].
pub struct HttpControllerMeta {
    controller: &'static str,
    // Computed by `#[routes]` through `nest_rs_codegen::snake_case`: a runtime
    // crate reaching `codegen` would pull `syn` into every app.
    token: &'static str,
    path: &'static str,
    versions: &'static [&'static str],
    routes: Vec<HttpRouteMeta>,
    mount: Arc<MountFn>,
}

/// The record `#[routes]` attaches for one controller; `mount` closes over the
/// handler wiring, and what it registers on the table by hand runs as a route.
pub fn http_controller_meta<F>(
    controller: &'static str,
    token: &'static str,
    path: &'static str,
    versions: &'static [&'static str],
    routes: Vec<HttpRouteMeta>,
    mount: F,
) -> HttpControllerMeta
where
    F: Fn(&Container, &mut HttpRoutes) + Send + Sync + 'static,
{
    HttpControllerMeta {
        controller,
        token,
        path,
        versions,
        routes,
        mount: Arc::new(mount),
    }
}

impl HttpControllerMeta {
    /// The controller struct's name (`UsersController`).
    pub fn controller(&self) -> &'static str {
        self.controller
    }

    /// The controller's name as an identifier fragment (`PostsController` →
    /// `posts`), the stem of each OpenAPI `operationId`; a name that is *only*
    /// the `Controller` suffix keeps it.
    pub fn token(&self) -> &'static str {
        self.token
    }

    /// The controller's shared path prefix (before URI versioning).
    pub fn path(&self) -> &'static str {
        self.path
    }

    /// `#[controller(version = …)]` — every version this controller serves, in
    /// declaration order. Empty means unversioned.
    pub fn versions(&self) -> &'static [&'static str] {
        self.versions
    }

    /// Each route this controller declares, in declaration order.
    pub fn routes(&self) -> &[HttpRouteMeta] {
        &self.routes
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
        match (route.versions(), version) {
            ([], _) => true,
            (_, None) => true,
            (declared, Some(v)) => declared.contains(&v),
        }
    }

    /// Register this controller's routes on `routes`, resolving handler
    /// dependencies from `container`.
    pub fn mount(&self, container: &Container, routes: &mut HttpRoutes) {
        (self.mount)(container, routes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
        let meta = http_controller_meta(
            "UsersController",
            "users",
            "/users",
            &[],
            Vec::new(),
            |_c, _r| {},
        );
        let mounted: Vec<_> = meta.mounted_versions().collect();
        assert_eq!(mounted, vec![None]);
        assert_eq!(meta.effective_prefix(None), "/users");
    }

    #[test]
    fn each_declared_version_is_its_own_mount_prefix() {
        let meta = http_controller_meta(
            "UsersController",
            "users",
            "/users",
            &["1", "2"],
            Vec::new(),
            |_c, _r| {},
        );
        let prefixes: Vec<_> = meta
            .mounted_versions()
            .map(|v| meta.effective_prefix(v))
            .collect();
        assert_eq!(prefixes, ["/v1/users", "/v2/users"]);
    }

    #[test]
    fn a_route_serves_every_controller_version_until_it_narrows_itself() {
        let mut parts = show_parts();
        assert!(
            HttpControllerMeta::serves(&http_route_meta(parts.clone()), Some("1")),
            "an undecorated route serves every version its controller declares",
        );
        parts.versions = &["2"];
        let route = http_route_meta(parts);
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

    fn show_parts() -> HttpRouteParts {
        HttpRouteParts {
            method: Method::GET,
            path: "/{id}",
            handler: "show",
            summary: Some("Fetch one"),
            description: None,
            tags: &["Users"],
            request_body: None,
            response: None,
            response_content_type: None,
            error_responses: &[],
            masked: false,
            path_params: &[],
            query_params: &[],
            header_params: &[],
            may_conflict: false,
            throttled: false,
            sets_location: false,
            sets_next_link: false,
            response_headers: &[],
            success_status: 200,
            scoped_guarded: false,
            public: false,
            deprecation: None,
            versions: &[],
        }
    }

    #[test]
    fn a_controller_reads_back_what_it_was_built_with() {
        let meta = http_controller_meta(
            "UsersController",
            "users",
            "/users",
            &["2"],
            vec![http_route_meta(show_parts())],
            |_c, _r| {},
        );
        assert_eq!(meta.controller(), "UsersController");
        assert_eq!(meta.token(), "users");
        assert_eq!(meta.path(), "/users");
        assert_eq!(meta.versions(), &["2"]);
        assert_eq!(meta.routes().len(), 1);
    }

    #[test]
    fn a_route_reads_back_each_fact_it_was_built_with() {
        fn schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
            generator.subschema_for::<String>()
        }
        let deprecation = DeprecationMeta {
            since: "2026-10-01",
            header: "@1790812800",
        };
        let route = http_route_meta(HttpRouteParts {
            method: Method::PUT,
            path: "/{id}",
            handler: "replace",
            versions: &["2"],
            summary: Some("Replace one"),
            description: Some("Every writable field."),
            tags: &["Users"],
            request_body: Some(RequestBodyMeta::Json(schema)),
            response: Some(schema),
            response_content_type: Some("text/plain"),
            error_responses: &[(503, schema)],
            masked: true,
            path_params: &[schema],
            query_params: &[schema, schema],
            header_params: &[schema, schema, schema],
            may_conflict: true,
            throttled: true,
            sets_location: true,
            sets_next_link: true,
            response_headers: &[("x-kind", "user")],
            success_status: 201,
            scoped_guarded: true,
            public: true,
            deprecation: Some(deprecation),
        });
        assert_eq!(route.method(), Method::PUT);
        assert_eq!(route.path(), "/{id}");
        assert_eq!(route.handler(), "replace");
        assert_eq!(route.versions(), &["2"]);
        assert_eq!(route.summary(), Some("Replace one"));
        assert_eq!(route.description(), Some("Every writable field."));
        assert_eq!(route.tags(), &["Users"]);
        assert_eq!(
            route.request_body().map(RequestBodyMeta::media_type),
            Some("application/json"),
        );
        assert!(route.response().is_some());
        assert_eq!(route.response_content_type(), Some("text/plain"));
        assert_eq!(route.error_responses()[0].0, 503);
        assert!(route.masked());
        assert_eq!(route.path_params().len(), 1);
        assert_eq!(route.query_params().len(), 2);
        assert_eq!(route.header_params().len(), 3);
        assert!(route.may_conflict());
        assert!(route.throttled());
        assert!(route.sets_location());
        assert!(route.sets_next_link());
        assert_eq!(route.response_headers(), &[("x-kind", "user")]);
        assert_eq!(route.success_status(), 201);
        assert!(route.scoped_guarded());
        assert!(route.public());
        assert_eq!(route.deprecation(), Some(deprecation));
    }

    #[test]
    fn mount_invokes_the_closure_with_the_container_and_routes() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let meta = http_controller_meta(
            "HealthController",
            "health",
            "/health",
            &[],
            Vec::new(),
            |_c, _r| {
                CALLS.fetch_add(1, Ordering::SeqCst);
            },
        );
        let container = Container::builder().build().expect("the container builds");
        let mut routes = HttpRoutes::new(None, None);

        meta.mount(&container, &mut routes);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        meta.mount(&container, &mut routes);
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }

    fn route(scoped_guarded: bool, public: bool) -> HttpRouteMeta {
        http_route_meta(HttpRouteParts {
            method: Method::POST,
            path: "/",
            handler: "create",
            summary: None,
            description: None,
            tags: &[],
            request_body: None,
            response: None,
            response_content_type: None,
            error_responses: &[],
            masked: false,
            path_params: &[],
            query_params: &[],
            header_params: &[],
            may_conflict: false,
            throttled: false,
            sets_location: false,
            sets_next_link: false,
            response_headers: &[],
            success_status: 200,
            scoped_guarded,
            public,
            deprecation: None,
            versions: &[],
        })
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
