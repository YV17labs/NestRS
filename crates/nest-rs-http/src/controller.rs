use std::sync::Arc;

use nest_rs_core::Container;
use poem::Route;

/// Implemented automatically by the `#[routes]` macro. Each controller
/// mounts its routes (prefixed with the controller's `PATH`) onto a parent
/// [`Route`].
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

/// The request body a route accepts: how it arrives on the wire, and the
/// schema of its content when the framework can name the type.
///
/// One value rather than a schema beside a media-type string, so the two can
/// never disagree — a document generator reads the media type off the variant
/// it matched instead of inferring one from a schema that may be absent.
///
/// The set is closed because the *framework* decides it: a body reaches a
/// handler through an extractor `#[routes]` recognizes, or through
/// `#[api(multipart = T)]`. A response's media type is the developer's to
/// declare, which is why
/// [`response_content_type`](HttpRouteMeta::response_content_type) is a plain
/// string and this is not.
#[derive(Clone, Copy)]
pub enum RequestBodyMeta {
    /// A `Json<T>` extractor — `application/json`, carrying `T`'s schema.
    Json(SchemaFn),
    /// A `multipart/form-data` body. `Some` when `#[api(multipart = T)]` names
    /// the type describing the form's parts; `None` for a bare
    /// [`poem::web::Multipart`] parameter, whose parts the handler pulls one by
    /// one and no type states — the document then says `multipart/form-data`
    /// with a free-form object, which is still more than silence.
    Multipart(Option<SchemaFn>),
    /// A `Form<T>` extractor — `application/x-www-form-urlencoded`, carrying
    /// `T`'s schema. Recognised because a route that binds one *has* a body:
    /// matching only `Json` and `Multipart` documented `POST /token` as taking
    /// no body at all, which is the shape RFC 6749 requires of an OAuth token
    /// endpoint.
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

    /// The schema builder for the body's content, when the body has a named
    /// type.
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
/// Built only by the `#[routes]` macro (struct literal) and read by the
/// framework's own discovery consumers — its fields are effectively an internal
/// ABI that versions in lockstep, not a stable hand-written surface. New facets
/// land here as public fields (`success_status`, `throttled`, …); a later
/// opaque-struct migration is slated to privatize the set behind accessors.
#[derive(Clone)]
pub struct HttpRouteMeta {
    /// The method this route answers.
    pub verb: HttpVerb,
    /// The route path, relative to the controller prefix.
    pub path: &'static str,
    /// The handler method's name — the `handler` field in the boot route log.
    pub handler: &'static str,
    /// `#[version("2")]` on the method — the versions this route serves, out of
    /// the ones its controller declares. **Empty means it serves every one of
    /// them**, which is what an undecorated route wants: a version is a
    /// controller-wide statement, and a route only opts *out* of part of it.
    ///
    /// The subset is checked at compile time by
    /// [`versions_declare`](crate::versions_declare), so a `#[version]` naming
    /// something `#[controller(version = …)]` never declared is an error at the
    /// route rather than a route that silently never mounts.
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
    /// its own [`Response`](poem::Response) (the `#[crud]` paginated list does).
    /// `None` only when neither applies.
    pub response: Option<SchemaFn>,
    /// The media type of the success response body, when it is **not**
    /// `application/json` — `#[api(response_content_type = "audio/mpeg")]` for
    /// a hand-built streamed [`Response`](poem::Response), or
    /// `text/event-stream` inferred from an `-> SSE` return.
    ///
    /// A free-form string rather than an enum, unlike
    /// [`RequestBodyMeta`]: what a handler streams back is the developer's
    /// contract with their client (`audio/mpeg`, `text/csv`,
    /// `application/octet-stream`), not a set the framework can close.
    pub response_content_type: Option<&'static str>,
    /// An ability shaper (`Authorize<_, _>`) masks this route's response, so a
    /// caller may receive a **subset** of [`response`](Self::response)'s
    /// properties — whichever ones its ability grants.
    ///
    /// The schema is published anyway. Publishing nothing was the honest
    /// reading of "the field set depends on the caller", and it typed every
    /// generated client's `#[crud]` response as `any` — losing the whole point
    /// of `#[expose]`, whose entity feeds the handler, the GraphQL schema and
    /// this document from one type. The document now carries the shape *and*
    /// says the fields are ability-dependent.
    pub masked: bool,
    /// Schema builders for the handler's `Path<T>` extractor components, in
    /// path order (a `Path<(A, B)>` tuple yields one per element). Empty when
    /// the handler binds its id another way (`Bind<_, _>`) — the doc then falls
    /// back to a `format: uuid` guess for id-like segments. Each imposes a
    /// `JsonSchema` bound, so a `Path<Uuid>` types the parameter as
    /// `string`/`format: uuid`, a `Path<i64>` as `integer`.
    pub path_params: &'static [SchemaFn],
    /// Schema builders for the handler's `Query<T>` extractor payloads (one per
    /// `Query<T>` argument; `Valid`/`Piped` wrappers are unwrapped). Each `T`'s
    /// object schema is expanded into one OpenAPI `query` parameter per property
    /// — this is how the `#[crud]` list op surfaces its `first`/`after`
    /// pagination cursor. Imposes `JsonSchema` on every `Query<T>` type, the
    /// same contract `Json<T>` bodies already carry.
    pub query_params: &'static [SchemaFn],
    /// Schema builders for the handler's [`Header<T>`](crate::Header) extractor
    /// payloads, expanded exactly like [`query_params`](Self::query_params) but
    /// into `in: header` parameters. A property absent from the schema's
    /// `required` is an optional header.
    pub header_params: &'static [SchemaFn],
    /// The operation is a write that can fail a uniqueness/constraint check and
    /// surface a `409 Conflict` — the `#[crud]` create/update/delete ops set it
    /// so the document advertises the conflict response their write-error mapper
    /// can actually produce. A hand-written handler leaves it `false`.
    pub may_conflict: bool,
    /// A `ThrottlerGuard` (controller- or method-level) rate-limits this route,
    /// so it can answer `429 Too Many Requests` with a `Retry-After` header —
    /// the OpenAPI document advertises that response (OAPI-O4). Detected by the
    /// guard's type name in `#[routes]`/`#[controller]` (the same name-based
    /// detection the masking-arm check uses); a hand-written handler that
    /// throttles by other means leaves it `false`.
    pub throttled: bool,
    /// The route's success response carries a `Location` header — `#[crud]`'s
    /// create names the row it just minted (RFC 9110 §15.3.2), `#[redirect]`
    /// names the target. The OpenAPI document declares the header so a
    /// generated client can read it, the same reason a throttled route's `429`
    /// declares `Retry-After`: a header that ships and is not declared is a
    /// header no generated client will ever look at.
    ///
    /// A hand-written handler that sets `Location` itself leaves this `false` —
    /// like [`throttled`](Self::throttled), it states what the framework knows
    /// it emitted, never what a handler might.
    pub sets_location: bool,
    /// The effective **success** HTTP status this route emits — `200` unless a
    /// `#[http_code(N)]` or `#[redirect(_, code)]` overrides it. Used by the
    /// OpenAPI document so the advertised success response matches the wire
    /// (OAPI-O3), instead of a hard-coded `200`.
    pub success_status: u16,
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
    /// The controller's name as an identifier fragment: the struct name with
    /// its `Controller` suffix dropped, snake_cased — `PostsController` →
    /// `posts`. A name that is *only* the suffix keeps it, because `_list`
    /// names nothing.
    ///
    /// The OpenAPI document builds `operationId` from it (`posts_list`), which
    /// a client generator turns into a method name. Computed by `#[routes]`
    /// through `nest_rs_codegen::snake_case` — the one casing rule the repo has
    /// — rather than at document time, for two reasons: the macro is where the
    /// type name is, and a runtime crate cannot reach `codegen` without dragging
    /// `syn` into every app's dependency graph. So the alternative is a second
    /// implementation of the same rule, which is what this replaced.
    pub token: &'static str,
    /// The controller's shared path prefix (before URI versioning).
    pub path: &'static str,
    /// `#[controller(version = …)]` — every version this controller serves, in
    /// declaration order. Empty means unversioned.
    ///
    /// A list rather than an `Option` because the common shape of a second API
    /// version is *most routes unchanged*: `version = ["1", "2"]` mounts the
    /// same handlers under both prefixes, and a route that differs opts out
    /// with its own `#[version]` instead of forcing a duplicate controller.
    pub versions: &'static [&'static str],
    /// Metadata for each route this controller declares.
    pub routes: Vec<HttpRouteMeta>,
    mount: Arc<MountFn>,
}

impl HttpControllerMeta {
    /// Assemble the discovery metadata for one controller. Emitted by the
    /// `#[controller]`/`#[routes]` macros; `mount` closes over the handler
    /// wiring.
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
    ///
    /// Every reader that composes a full path — the boot log, the OpenAPI
    /// document, the transport's own prefix collection — iterates this, so
    /// "how many addresses does this controller have" has one answer.
    pub fn mounted_versions(&self) -> impl Iterator<Item = Option<&'static str>> + '_ {
        // `[None]` rather than an empty iterator: an unversioned controller
        // still mounts, at one address. Yielding nothing would silently unmount
        // every controller that declares no version.
        let unversioned = self.versions.is_empty().then_some(None);
        unversioned
            .into_iter()
            .chain(self.versions.iter().map(|v| Some(*v)))
    }

    /// Mount prefix for one of [`mounted_versions`](Self::mounted_versions)
    /// (`/v1/users`, or `/users` for `None`). Readers composing full route
    /// paths join each route onto this so they match what
    /// [`mount`](Self::mount) serves.
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
    fn request_body_meta_pairs_each_media_type_with_its_own_schema() {
        // The variant *is* the media type, so a document generator never has to
        // infer one — including for the untyped multipart body, which carries a
        // media type and no schema.
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
        // The `[None]` in `mounted_versions` is load-bearing: yielding nothing
        // for an empty version list would unmount every controller that
        // declares no version.
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
        // `version_path` joins `/v<v>` ahead of the controller path — the
        // single place URI versioning lives, so this is the contract.
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
        // The `const fn` behind the `#[version]` compile assertion.
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
        // The mount closure is the seam `#[routes]` emits; assert it's called
        // exactly once per `mount` invocation and receives the same container.
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
            success_status: 200,
            scoped_guarded,
            public,
            versions: &[],
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
