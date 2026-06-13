//! HTTP attribute macros, re-exported by `nestrs-http`. Generated code uses
//! absolute paths (`::nest_rs_http::*`, `::poem::*`, `::nest_rs_core::*`), so
//! this crate has no dependency on its surface crate — they resolve at the
//! call site.

use proc_macro::TokenStream;

mod attr;
mod controller;
mod crud;
mod input;
mod interceptor;
mod response;
mod routes;

/// `#[controller(path = "/health")]` — paired with `#[routes]` on the impl
/// block. Generates `from_container(&Container) -> Self` and a `pub const PATH`.
///
/// Class-level `#[use_guards(...)]` / `#[use_filters(...)]` /
/// `#[use_interceptors(...)]` placed *below* `#[controller]` apply to every
/// route the controller mounts; they stack *outside* any per-route binding
/// (first listed outermost). An optional `version = "1"` enables URI versioning
/// — see [`version_path`](::nest_rs_http::version_path).
///
/// The `Discoverable` impl is emitted by `#[routes]` (which owns the route
/// table), not here.
///
/// # Expands to
///
/// An inherent `impl` carrying the path/version consts, `from_container`, and
/// hidden helper fns `#[routes]` reads (injected keys + the per-family
/// controller-level layer specs). Illustrative sketch:
///
/// ```ignore
/// struct UsersController { /* … */ }                 // the item, unchanged
///
/// impl UsersController {
///     pub const PATH: &'static str = "/users";
///     pub const VERSION: Option<&'static str> = Some("1");   // from `version = "1"`
///     fn from_container(c: &::nest_rs_core::Container) -> Self { /* … */ }
///
///     // read by `#[routes]` for the access graph + per-route layer pools:
///     #[doc(hidden)] fn __nestrs_injected() -> Vec<TypeId> { /* #[inject] + layer keys */ }
///     #[doc(hidden)] fn __nestrs_controller_guard_specs()  -> Vec<ScopedGuardSpec>  { /* … */ }
///     #[doc(hidden)] fn __nestrs_controller_interceptor_specs() -> Vec<…> { /* … */ }
///     #[doc(hidden)] fn __nestrs_controller_filter_specs() -> Vec<…> { /* … */ }
///     #[doc(hidden)] fn __nestrs_controller_pipe_specs()   -> Vec<…> { /* … */ }
///     #[doc(hidden)] fn __nestrs_controller_exception_filter_specs() -> Vec<…> { /* … */ }
/// }
/// ```
#[proc_macro_attribute]
pub fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    controller::controller(args, input)
}

/// Mark a struct as a **global** HTTP interceptor. Behaves like `#[injectable]`
/// for construction and additionally emits a `Discoverable` impl attaching an
/// `HttpEndpointWrap`; the HTTP transport reads those metas at boot. The
/// struct must implement `nest_rs_interceptors::Interceptor`. An optional
/// `priority = <int>` orders the wrap among the endpoint wraps (defaults to the
/// interceptor band).
///
/// # Expands to
///
/// Like `#[injectable]`, but `register` attaches an `HttpEndpointWrap` meta
/// instead of providing the value — so the type is mounted automatically, not
/// resolved as a provider. Illustrative sketch:
///
/// ```ignore
/// struct TracingInterceptor { /* … */ }              // the item, unchanged
///
/// impl TracingInterceptor { fn from_container(c: &Container) -> Self { /* … */ } }
///
/// impl ::nest_rs_core::Discoverable for TracingInterceptor {
///     // dependencies / dependency_names / optional_dependencies / injected — as #[injectable]
///     fn register(b: ContainerBuilder) -> ContainerBuilder {
///         let arc: Arc<dyn ::nest_rs_interceptors::Interceptor> =
///             Arc::new(Self::from_container(&b.snapshot()));
///         b.attach_meta::<Self, ::nest_rs_http::HttpEndpointWrap>(
///             HttpEndpointWrap::with_priority(PRIORITY, move |_c, ep| /* wrap ep with arc */),
///         )
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn interceptor(args: TokenStream, input: TokenStream) -> TokenStream {
    interceptor::interceptor(args, input)
}

/// Bind controller methods to HTTP routes. Applied to an `impl` block
/// belonging to a `#[controller]`-marked struct. Each method tagged with
/// `#[get("/path")]`, `#[post]`, `#[put]`, `#[delete]`, or `#[patch]` is wired
/// as a poem handler.
///
/// Per-method attributes (all consumed; no imports needed):
///
/// - `#[use_guards(...)]` — container-resolved guards, first listed outermost.
/// - `#[use_filters(...)]` — exception filters, wrap *outside* the guards.
/// - `#[use_interceptors(...)]` — container-resolved interceptors.
/// - `#[meta(EXPR)]` (repeatable) — typed metadata read back by a guard with
///   `nest_rs_http::Reflector` (value type: `Clone + Send + Sync + 'static`).
/// - `#[api(summary, description, tags(...))]` — OpenAPI facets.
///
/// The macro also reads each handler's signature and records the schema of any
/// `Json<T>` request body / response into the route's `HttpRouteMeta` (`T:
/// nest_rs_http::schemars::JsonSchema`); raw `Response`/`String` returns carry
/// no schema.
///
/// Emits `nest_rs_http::Controller` (mount entry point) and
/// `nest_rs_core::Discoverable` (attaches the route table + mount closure).
///
/// # Expands to
///
/// The impl block (verb/layer/response attrs stripped), one `#[poem::handler]`
/// wrapper per route, `impl Controller` (builds the sub-`Route`, folding each
/// route's guard/pipe/filter/interceptor pools via `RouteShaper` +
/// `wrap_route_*`), and `impl Discoverable` whose `register` attaches an
/// `HttpControllerMeta` (the route table). Illustrative sketch:
///
/// ```ignore
/// impl UsersController { /* methods, verb attrs removed */ }
///
/// #[::poem::handler]
/// async fn __nestrs_route_list(Data(ctrl): Data<&Arc<UsersController>> /* extractors */)
///     -> /* return type or ::poem::Result<Response> when response shapers apply */
/// { ctrl.list(/* forwarded args */).await }
///
/// impl ::nest_rs_http::Controller for UsersController {
///     fn mount(c: &Container, route: ::poem::Route) -> ::poem::Route {
///         let ctrl = Arc::new(Self::from_container(c));
///         let sub = ::poem::Route::new()
///             .at("/", ::poem::get(/* RouteShaper-wrapped __nestrs_route_list */))   // per path
///             .data(ctrl);
///         route.nest(version_path(Self::VERSION, Self::PATH).as_str(), sub)
///     }
/// }
///
/// impl ::nest_rs_core::Discoverable for UsersController {
///     fn injected() -> Vec<TypeId> { /* #[inject] + every per-route layer key */ }
///     fn register(b: ContainerBuilder) -> ContainerBuilder {
///         b.attach_meta::<Self, ::nest_rs_http::HttpControllerMeta>(
///             HttpControllerMeta::new(tag, Self::PATH, Self::VERSION, vec![/* HttpRouteMeta… */], mount),
///         )
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn routes(args: TokenStream, input: TokenStream) -> TokenStream {
    routes::routes(args, input)
}

/// Generate standard REST operations (list/get/create/update/delete) on a
/// `#[controller]` impl block, re-emitting under `#[routes]`. Grammar:
/// `#[crud(entity = …::Entity, output = Dto, create = CreateDto,
/// update = UpdateDto, readonly, paginate = cursor|page|none)]`.
///
/// The generated list is **keyset-paginated by default** (`?first=&after=`,
/// next cursor echoed in `x-next-cursor`, body a plain maskable array);
/// `paginate = none` opts out into the full collection, backstopped by
/// `CrudService::list`'s hard cap.
///
/// Guards are declared once on the controller (`#[use_guards(...)]` on the
/// struct) — every generated route inherits them. A hand-written
/// `list`/`get`/`create`/`update`/`delete` method overrides its generated
/// counterpart.
///
/// # Expands to
///
/// The missing CRUD methods are synthesized onto the impl block (each
/// delegating to `CrudService` and carrying its own verb + `#[api]` attrs),
/// then the whole block is re-emitted under `#[routes]` — so the final shape is
/// `#[routes]`'s (see its docs). Also emits a hidden per-controller error-map
/// fn. Illustrative sketch:
///
/// ```ignore
/// #[::nest_rs_http::routes]
/// impl UsersController {
///     #[get("/")]   #[api(summary = "List Users", tags("User"))]
///     async fn list(&self, _authz: Authorize<Read, Entity>, page: Query<PageParams>) -> Result<Response> {
///         let p = CrudService::page(&*self.svc, page.limit(), page.after_uuid())
///             .await.map_err(__nestrs_crud_internal_UsersController)?;
///         // Json(Vec<Dto>) + `x-next-cursor` header when p.next_cursor is Some
///     }
///     // get → CrudService::access(Read, id); create/update/delete unless `readonly`,
///     // each guarded by Authorize<Action, Entity> and mapping Access::{Denied=>403,Missing=>404}
///     // … plus any hand-written methods (which override the generated ones)
/// }
///
/// #[doc(hidden)]
/// fn __nestrs_crud_internal_UsersController<E: ToString>(e: E) -> ::poem::Error { /* 500 */ }
/// ```
#[proc_macro_attribute]
pub fn crud(args: TokenStream, input: TokenStream) -> TokenStream {
    crud::entry(args, input)
}

/// `#[input]` — shorthand for input DTOs. Appends
/// `#[derive(::serde::Deserialize, ::validator::Validate)]` and
/// `#[serde(deny_unknown_fields)]` so an unknown field on the wire
/// (e.g. `is_admin: true`) is rejected at parse time instead of silently
/// dropped.
///
/// # Expands to
///
/// The struct, with the derives + serde attribute prepended (stacking with any
/// existing `#[derive(...)]`):
///
/// ```ignore
/// #[derive(::serde::Deserialize, ::validator::Validate)]
/// #[serde(deny_unknown_fields)]
/// struct CreateUser { /* … */ }
/// ```
#[proc_macro_attribute]
pub fn input(args: TokenStream, item: TokenStream) -> TokenStream {
    input::input(args, item)
}

/// `#[http_code(N)]` — override the response status (`100..=999`). Passthrough
/// marker consumed by `#[routes]`. Mutually exclusive with `#[redirect]`.
///
/// # Expands to
///
/// Nothing on its own — the attribute entry returns the item unchanged. The
/// real effect lives in `#[routes]`, which drains the marker and wraps the
/// handler's success path so the emitted wrapper sets the status:
/// `__response.set_status(StatusCode::from_u16(N)?)` (the `Err` path keeps its
/// own status).
#[proc_macro_attribute]
pub fn http_code(args: TokenStream, item: TokenStream) -> TokenStream {
    response::passthrough(args, item)
}

/// `#[response_header("name", "value")]` — append a header to the response.
/// Stacks with `#[http_code]` and `#[redirect]`; repeatable. Passthrough
/// marker consumed by `#[routes]`.
///
/// # Expands to
///
/// Nothing on its own — returns the item unchanged. `#[routes]` drains the
/// marker and emits a header write on the handler's success path:
/// `__response.headers_mut().insert(HeaderName::from_static("name"),
/// HeaderValue::from_static("value"))` — `set-cookie` uses `.append()` so it
/// stacks instead of overriding.
#[proc_macro_attribute]
pub fn response_header(args: TokenStream, item: TokenStream) -> TokenStream {
    response::passthrough(args, item)
}

/// `#[redirect("url"[, code])]` — discard the handler's payload and return a
/// redirect. Status defaults to `307` and must be in `300..=399`. Mutually
/// exclusive with `#[http_code]`. The decorated method's body must be empty
/// — `#[routes]` does not call it. Passthrough marker consumed by `#[routes]`.
///
/// # Expands to
///
/// Nothing on its own — returns the item unchanged. `#[routes]` drains the
/// marker and replaces the handler body entirely (the user method is never
/// called): it builds a redirect response, e.g.
/// `Response::builder().status(StatusCode::from_u16(307)?).header(LOCATION,
/// "url").finish()`, then applies any stacked `#[response_header]`.
#[proc_macro_attribute]
pub fn redirect(args: TokenStream, item: TokenStream) -> TokenStream {
    response::passthrough(args, item)
}
