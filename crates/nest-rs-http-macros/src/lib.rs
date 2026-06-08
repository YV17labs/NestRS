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
#[proc_macro_attribute]
pub fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    controller::controller(args, input)
}

/// Mark a struct as a **global** HTTP interceptor. Behaves like `#[injectable]`
/// for construction and additionally emits a `Discoverable` impl attaching an
/// `HttpEndpointWrap`; the HTTP transport reads those metas at boot. The
/// struct must implement `nest_rs_interceptors::Interceptor`.
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
#[proc_macro_attribute]
pub fn routes(args: TokenStream, input: TokenStream) -> TokenStream {
    routes::routes(args, input)
}

/// Generate standard REST operations (list/get/create/update/delete) on a
/// `#[controller]` impl block, re-emitting under `#[routes]`. Grammar:
/// `#[crud(entity = …::Entity, output = Dto, create = CreateDto,
/// update = UpdateDto, readonly, paginate = cursor|page)]`.
///
/// Guards are declared once on the controller (`#[use_guards(...)]` on the
/// struct) — every generated route inherits them. A hand-written
/// `list`/`get`/`create`/`update`/`delete` method overrides its generated
/// counterpart.
#[proc_macro_attribute]
pub fn crud(args: TokenStream, input: TokenStream) -> TokenStream {
    crud::entry(args, input)
}

/// `#[input]` — shorthand for input DTOs. Appends
/// `#[derive(::serde::Deserialize, ::validator::Validate)]` and
/// `#[serde(deny_unknown_fields)]` so an unknown field on the wire
/// (e.g. `is_admin: true`) is rejected at parse time instead of silently
/// dropped.
#[proc_macro_attribute]
pub fn input(args: TokenStream, item: TokenStream) -> TokenStream {
    input::input(args, item)
}

/// `#[http_code(N)]` — override the response status (`100..=999`). Passthrough
/// marker consumed by `#[routes]`. Mutually exclusive with `#[redirect]`.
#[proc_macro_attribute]
pub fn http_code(args: TokenStream, item: TokenStream) -> TokenStream {
    response::passthrough(args, item)
}

/// `#[response_header("name", "value")]` — append a header to the response.
/// Stacks with `#[http_code]` and `#[redirect]`; repeatable. Passthrough
/// marker consumed by `#[routes]`.
#[proc_macro_attribute]
pub fn response_header(args: TokenStream, item: TokenStream) -> TokenStream {
    response::passthrough(args, item)
}

/// `#[redirect("url"[, code])]` — discard the handler's payload and return a
/// redirect. Status defaults to `307` and must be in `300..=399`. Mutually
/// exclusive with `#[http_code]`. The decorated method's body must be empty
/// — `#[routes]` does not call it. Passthrough marker consumed by `#[routes]`.
#[proc_macro_attribute]
pub fn redirect(args: TokenStream, item: TokenStream) -> TokenStream {
    response::passthrough(args, item)
}
