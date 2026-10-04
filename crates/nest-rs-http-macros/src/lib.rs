//! HTTP attribute macros, re-exported by `nest-rs-http`. Generated code uses
//! absolute paths (`::nest_rs_http::*`, `::nest_rs_http::poem::*`, `::nest_rs_core::*`), so
//! this crate has no dependency on its surface crate — they resolve at the
//! call site.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod attr;
mod controller;
mod crud;
mod interceptor;
mod response;
mod routes;

/// Generates `pub const PATH`, `pub const VERSIONS` (empty when unversioned)
/// and `from_container(&Container) -> Self`.
///
/// Class-level `#[use_guards(...)]` / `#[use_filters(...)]` /
/// `#[use_interceptors(...)]` placed *below* `#[controller]` apply to every
/// route the controller mounts; they stack *outside* any per-route binding
/// (first listed outermost). An optional `version = "1"` enables URI versioning
/// — see `version_path`.
///
/// The `Discoverable` impl is emitted by `#[routes]` (which owns the route
/// table), not here.
///
/// # Expands to
///
/// The item unchanged, plus an inherent `impl` carrying the consts and
/// `from_container`; the `#[doc(hidden)]` helpers beside them are what
/// `#[routes]` reads (the injected keys and the controller-level
/// layer specs).
#[proc_macro_attribute]
pub fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(controller::controller(args, input).into()).into()
}

/// Behaves like `#[injectable]` for construction and additionally emits a
/// `Discoverable` impl attaching an `HttpEndpointWrap`; the HTTP transport
/// reads those metas at boot. An optional `priority = <int>` orders the wrap
/// among the endpoint wraps (defaults to the interceptor band).
///
/// # Expands to
///
/// Like `#[injectable]`, but `register` attaches an `HttpEndpointWrap` meta
/// instead of providing the value — so the type is mounted automatically, not
/// resolved as a provider.
#[proc_macro_attribute]
pub fn interceptor(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(interceptor::interceptor(args, input).into()).into()
}

/// Applied to an `impl` block belonging to a `#[controller]`-marked struct.
/// Each method tagged with `#[get("/path")]`, `#[post]`, `#[put]`,
/// `#[delete]`, or `#[patch]` is wired as a poem handler.
///
/// Per-method attributes (all consumed; no imports needed):
///
/// - `#[authorize(Action, Entity)]` — the route's authz posture, uniform with
///   `#[resolver]`'s. Desugars to the `nest_rs_authz::http::Authorize<A, E>`
///   extractor as the handler's first parameter: class gate before the body,
///   automatic response masking after it. Mutually exclusive with `#[public]`.
/// - `#[public]` — the route is reachable anonymously; global guards still run
///   and read the marker.
/// - `#[use_guards(...)]` — container-resolved guards, first listed outermost.
/// - `#[use_filters(...)]` — container-resolved error-mapping filters; they
///   wrap *inside* the guards, so a denial short-circuits before them.
/// - `#[use_exception_filters(...)]` — container-resolved typed catches, tried
///   innermost of the response families.
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
/// route's guard/pipe/filter/interceptor pools), and `impl Discoverable` whose
/// `register` attaches an `HttpControllerMeta` — the route table the transport
/// mounts.
#[proc_macro_attribute]
pub fn routes(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(routes::routes(args, input).into()).into()
}

/// Grammar: `#[crud(entity = …::Entity, output = Dto, create = CreateDto,
/// update = UpdateDto, ops = [list, get, ...], paginate = cursor|none)]`.
///
/// `ops` selects which operations to generate; omit it for all five
/// (back-compatible). A write op is generated only when the resource genuinely
/// offers it: `create`/`update` require their input type **and** that the
/// service implements `Creatable`/`Updatable`; `delete` requires `Deletable`.
/// Listing `ops = [create]` without `create = <Type>` is a compile error, not a
/// silently dropped (or no-op) route — so a resource never exposes a write it
/// does not have.
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
/// delegating to `CrudService`, taking the `Authorize<Action, Entity>`
/// extractor `#[authorize(Action, Entity)]` desugars to, and carrying its own
/// verb + `#[api]` attrs), then the whole block is re-emitted under `#[routes]`
/// — so the final shape is `#[routes]`'s. A write failure maps through `crud_error` to
/// 409/403/404, logging an unexpected `DbErr` and shipping an empty-bodied 500.
#[proc_macro_attribute]
pub fn crud(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(crud::entry(args, input).into()).into()
}

/// `#[http_code(N)]` — override the response status (`100..=999`). A marker
/// consumed by `#[routes]`, written bare or path-qualified. Mutually exclusive
/// with `#[redirect]`.
///
/// # Expands to
///
/// On its own, a compile error: `#[routes]` removes every marker it reads, so
/// one that reaches this entry sits outside a `#[routes]` impl or under an
/// alias, and would shape nothing. The real effect lives in `#[routes]`, which drains the marker and wraps the
/// handler's success path so the emitted wrapper sets the status:
/// `__response.set_status(StatusCode::from_u16(N)?)` (the `Err` path keeps its
/// own status).
#[proc_macro_attribute]
pub fn http_code(_args: TokenStream, item: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(response::unread("http_code", item).into()).into()
}

/// `#[response_header("name", "value")]` — append a header to the response.
/// Stacks with `#[http_code]` and `#[redirect]`; repeatable. A marker consumed
/// by `#[routes]`, written bare or path-qualified.
///
/// # Expands to
///
/// On its own, a compile error naming `#[routes]` (see `#[http_code]`).
/// `#[routes]` drains the
/// marker and emits a header write on the handler's success path:
/// `__response.headers_mut().insert(HeaderName::from_static("name"),
/// HeaderValue::from_static("value"))` — `set-cookie` uses `.append()` so it
/// stacks instead of overriding.
#[proc_macro_attribute]
pub fn response_header(_args: TokenStream, item: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(response::unread("response_header", item).into()).into()
}

/// `#[redirect("url"[, code])]` — discard the handler's payload and return a
/// redirect. Status defaults to `307` and must be in `300..=399`. Mutually
/// exclusive with `#[http_code]`. The decorated method's body must be empty
/// — `#[routes]` does not call it. A marker consumed by `#[routes]`, written
/// bare or path-qualified.
///
/// # Expands to
///
/// On its own, a compile error naming `#[routes]` (see `#[http_code]`).
/// `#[routes]` drains the
/// marker and replaces the handler body entirely (the user method is never
/// called): it builds a redirect response, e.g.
/// `Response::builder().status(StatusCode::from_u16(307)?).header(LOCATION,
/// "url").finish()`, then applies any stacked `#[response_header]`.
#[proc_macro_attribute]
pub fn redirect(_args: TokenStream, item: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(response::unread("redirect", item).into()).into()
}
