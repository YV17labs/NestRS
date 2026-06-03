//! HTTP decorator macros, re-exported by `nestrs-http`. Generated code uses
//! absolute paths (`::nestrs_http::*`, `::poem::*`, `::nestrs_core::*`), so
//! this crate has no dependency on its surface crate — they resolve at the
//! call site.

use proc_macro::TokenStream;

mod attr;
mod controller;
mod crud;
mod interceptor;
mod routes;

/// `#[controller(path = "/health")]` — paired with `#[routes]` on the impl
/// block. Generates `from_container(&Container) -> Self` and a `pub const PATH`.
///
/// Class-level `#[use_guards(...)]` / `#[use_filters(...)]` /
/// `#[use_interceptors(...)]` placed *below* `#[controller]` apply to every
/// route the controller mounts; they stack *outside* any per-route binding
/// (first listed outermost). An optional `version = "1"` enables URI versioning
/// — see [`version_path`](::nestrs_http::version_path).
///
/// The `Discoverable` impl is emitted by `#[routes]` (which owns the route
/// table), not here.
#[proc_macro_attribute]
pub fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    controller::controller(args, input)
}

/// Mark a struct as a **global** HTTP interceptor. Behaves like `#[injectable]`
/// for construction and additionally emits a `Discoverable` impl attaching an
/// `HttpInterceptorMeta`; the HTTP transport reads those metas at boot. The
/// struct must implement `nestrs_middleware::Interceptor`.
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
///   `nestrs_http::Reflector` (value type: `Clone + Send + Sync + 'static`).
/// - `#[api(summary, description, tags(...))]` — OpenAPI facets.
///
/// The macro also reads each handler's signature and records the schema of any
/// `Json<T>` request body / response into the route's `HttpRouteMeta` (`T:
/// nestrs_http::schemars::JsonSchema`); raw `Response`/`String` returns carry
/// no schema.
///
/// Emits `nestrs_http::Controller` (mount entry point) and
/// `nestrs_core::Discoverable` (attaches the route table + mount closure).
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
