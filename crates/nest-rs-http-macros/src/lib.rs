//! HTTP attribute macros, re-exported by `nest-rs-http`; the generated paths
//! (`::nest_rs_http::*`, `::nest_rs_core::*`) resolve at the call site.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod attr;
mod controller;
mod crud;
mod response;
mod routes;

/// Generates `pub const PATH`, `pub const VERSIONS` (empty when unversioned)
/// and a hidden constructor the boot builds the controller with.
///
/// Class-level `#[use_guards(...)]` / `#[use_interceptors(...)]` /
/// `#[use_exception_filters(...)]` placed *below* `#[controller]` apply to
/// every route the controller mounts; they run *outside* any per-route binding
/// (first listed outermost), a controller's exception filters after the
/// route's own. A `#[meta(EXPR)]` below it is read by every route,
/// unless the route declares a value of the same type. An optional
/// `version = "1"` enables URI versioning.
/// The `Discoverable` impl is emitted by `#[routes]`.
///
/// # Expands to
///
/// The item unchanged, plus an inherent `impl` carrying the consts, a hidden
/// constructor and the `#[doc(hidden)]` helpers `#[routes]` reads.
#[proc_macro_attribute]
pub fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(controller::controller(args, input).into()).into()
}

/// Applied to an `impl` block belonging to a `#[controller]`-marked struct.
/// Each method tagged with `#[get("/path")]`, `#[post]`, `#[put]`,
/// `#[delete]`, or `#[patch]` is wired as a route of the HTTP vocabulary:
/// every parameter a `nest_rs_http::FromRequest`, the return an
/// `IntoResponse` or a `Result` of one whose error converts into an
/// `HttpError`.
///
/// Per-method attributes (all consumed; no imports needed):
///
/// - `#[authorize(Action, Entity)]` — the route's authz posture: the class
///   gate `nest_rs_authz::http::Authorize<A, E>` declares
///   (`nest_rs_authz::class_gate`), run right after the guards and before any
///   interceptor or filter the route declares, and its response masking.
///   Mutually exclusive with `#[public]`.
/// - `#[public]` — the route is reachable anonymously; global guards still run
///   and read the marker.
/// - `#[use_guards(...)]` — container-resolved guards, first listed outermost.
/// - `#[use_exception_filters(...)]` — container-resolved
///   `nest_rs_core::ExceptionFilter`s, tried before the controller's and the
///   app's on a failure of the handler stage; a guard's refusal reaches none.
/// - `#[use_interceptors(...)]` — container-resolved `nest_rs_core::Interceptor`s,
///   after the guards and the class gate, inside the app's and the
///   controller's.
/// - `#[meta(EXPR)]` (repeatable) — typed metadata read back by a guard with
///   `nest_rs_http::Reflector`, over a value of the same type the controller
///   declares (value type: `Send + Sync + 'static`; evaluated once per process,
///   on the first read, so an expression reading runtime state reads it once).
/// - `#[api(summary, description, tags(...), response, multipart,
///   response_content_type, error(...), deprecated)]` — OpenAPI facets;
///   `deprecated = "YYYY-MM-DD"` also stamps the `Deprecation` header (RFC 9745)
///   on every answer. Rust's own `#[deprecated]` on a handler is refused.
///
/// The schema of any `Json<T>` request body / response is recorded into the
/// route's `HttpRouteMeta` (`T: nest_rs_http::schemars::JsonSchema`); raw
/// `Response`/`String` returns carry no schema.
///
/// # Expands to
///
/// The impl block (verb/layer/response attrs stripped), one wrapper reading
/// the handler's parameters off the request it borrows and one `static`
/// pipeline site per route, and `impl nest_rs_core::Discoverable` attaching
/// the `HttpControllerMeta` route table the transport mounts — its mount
/// registering each route on the transport's `HttpRoutes`, its site's chains
/// composed against the container — and declaring, per parameter type, the
/// provider an extractor resolves per request (`Scoped<T>`, `Bind<A, S>`),
/// which the boot's access graph checks.
#[proc_macro_attribute]
pub fn routes(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(routes::routes(args, input).into()).into()
}

/// Grammar: `#[crud(entity = …::Entity, output = Dto, create = CreateDto,
/// update = UpdateDto, ops = [list, get, ...], paginate = cursor|none)]`.
///
/// `ops` selects which operations to generate; omit it for all five. A write
/// op is generated only when the resource offers it: `create`/`update` require
/// their input type **and** that the service implements
/// `Creatable`/`Updatable`; `delete` requires `Deletable`. Listing
/// `ops = [create]` without `create = <Type>` is a compile error.
///
/// The generated list is **keyset-paginated by default** (`?first=&after=`,
/// the next page linked by `Link: <…>; rel="next"`, body a plain maskable array);
/// `paginate = none` opts out into the full collection, backstopped by
/// `CrudService::list`'s hard cap.
///
/// Guards are declared once on the controller (`#[use_guards(...)]` on the
/// struct) and every generated route inherits them. A hand-written
/// `list`/`get`/`create`/`update`/`delete` method overrides its generated
/// counterpart.
///
/// # Expands to
///
/// The missing CRUD methods synthesized onto the impl block (each delegating
/// to `CrudService` behind `#[authorize(Action, Entity)]`, with its own verb
/// and `#[api]` attrs), the whole block re-emitted under `#[routes]`. A write
/// failure maps to 409/403/404; an unexpected `DbErr` is logged and answers an
/// empty-bodied 500.
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
/// On its own, a compile error: a marker reaching this entry sits outside a
/// `#[routes]` impl or under an alias. Under `#[routes]`, the handler's success
/// path sets the status; the `Err` path keeps its own.
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
/// On its own, a compile error naming `#[routes]`. Under `#[routes]`, a header
/// insert on the handler's success path; `set-cookie` appends instead.
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
/// On its own, a compile error naming `#[routes]`. Under `#[routes]`, a handler
/// that never calls the method and answers the redirect, with any stacked
/// `#[response_header]` applied.
#[proc_macro_attribute]
pub fn redirect(_args: TokenStream, item: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(response::unread("redirect", item).into()).into()
}
