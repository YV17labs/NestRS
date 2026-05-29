//! HTTP decorator macros, re-exported by `nestrs-http` so apps write
//! `nestrs_http::controller` etc. The generated code uses absolute paths
//! (`::nestrs_http::*`, `::poem::*`, `::nestrs_core::*`), so this crate does
//! not depend on those crates — they resolve at the call site.
//!
//! Each `#[proc_macro_attribute]` here is a thin entry that the language requires
//! at the crate root; the implementation lives in the topical submodules
//! (`controller`, `routes`, `interceptor`, `crud`, `attr`).

use proc_macro::TokenStream;

mod attr;
mod controller;
mod crud;
mod interceptor;
mod routes;

/// `#[controller(path = "/health")]` — paired with `#[routes]` on the impl block.
///
/// Generates a `from_container(&Container) -> Self` constructor and a
/// `pub const PATH: &'static str` used by `#[routes]` as the route prefix.
///
/// A `#[use_guards(GuardA, GuardB)]` attribute placed *on the controller struct*
/// (just below `#[controller]`) declares **controller-level guards** — the same
/// decorator the verb attributes use per route, now at the class level (the
/// `@UseGuards` analog). They run before *every* route the controller mounts —
/// hand-written and `#[crud]`-generated alike — so security is declared once per
/// feature, not repeated per route. They stack *outside* any per-route
/// `#[use_guards]` (both run; first listed outermost), never replacing them.
/// `#[use_guards]` must sit below `#[controller]`: it is an inert attribute that
/// `#[controller]` consumes and strips.
///
/// An optional `version = "1"` enables **URI API versioning** for the whole
/// controller: every route mounts under `/v1` (the NestJS `@Version` analog with
/// `VersioningType.URI`). The version segment flows into the route log and the
/// OpenAPI document too — see [`version_path`](::nestrs_http::version_path).
///
/// The `Discoverable` impl is emitted by `#[routes]` rather than here — it
/// needs the route table that `#[routes]` collects, and emitting it in two
/// places would conflict.
#[proc_macro_attribute]
pub fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    controller::controller(args, input)
}

/// Mark a struct as an HTTP interceptor that the framework will discover
/// and wrap around the route tree.
///
/// Behaves like `#[injectable]` for construction (fields with `#[inject]`
/// pulled from the container, others default), and additionally emits an
/// `impl Discoverable` that attaches an `HttpInterceptorMeta` describing
/// this type. The HTTP transport reads those metas via
/// `DiscoveryService::meta::<HttpInterceptorMeta>()` at boot.
///
/// The struct must implement `nestrs_middleware::Interceptor` — the macro
/// emits an `Arc<dyn Interceptor>` cast that fails at compile time if it
/// does not.
#[proc_macro_attribute]
pub fn interceptor(args: TokenStream, input: TokenStream) -> TokenStream {
    interceptor::interceptor(args, input)
}

/// Bind controller methods to HTTP routes.
///
/// Applied to an `impl` block belonging to a `#[controller]`-marked struct.
/// Each method tagged with `#[get("/path")]`, `#[post("/path")]`, `#[put]`,
/// `#[delete]` or `#[patch]` is wired as a poem handler. Method signatures
/// keep `&self` plus any poem extractors (`Path<T>`, `Json<T>`, `Query<T>`...).
///
/// Tag a method with `#[use_guards(GuardA, GuardB)]` to run those guards before
/// it — each is resolved from the container (so a guard is an `#[injectable]`
/// provider that can inject its own dependencies) and the first listed runs
/// outermost. A guard may attach request-scoped context the handler reads back
/// via `nestrs_http::Ctx<T>`. Like the verb attributes, `#[use_guards]` is
/// consumed here and needs no import.
///
/// Tag a method with `#[use_filters(FilterA, FilterB)]` to bind exception filters
/// to just that route (the `@UseFilters` analog; `HttpTransport::filter` is the
/// global form). Each is resolved from the container and wraps the handler
/// *outside* its guards, so it maps an error from the handler or a guard into a
/// response. Consumed here like `#[use_guards]`, no import needed.
///
/// Tag a method with `#[meta(EXPR)]` (repeatable) to attach a typed metadata
/// value to the route — the `@SetMetadata` / `@Roles` analog. `EXPR` is
/// evaluated once at mount and inserted into the request just outside the
/// route's guards, so a `#[use_guards]` guard reads it back with
/// `nestrs_http::Reflector::new(req).get::<T>()` to vary its decision. The value
/// type must be `Clone + Send + Sync + 'static`. Like `#[use_guards]`, the
/// attribute is consumed here and needs no import.
///
/// Tag a method with `#[api(summary = "...", description = "...", tags("a",
/// "b"))]` to enrich its OpenAPI operation (the analog of NestJS's
/// `@ApiOperation` / `@ApiTags`); every field is optional and, like
/// `#[use_guards]`, the attribute is consumed here. Independently, the macro
/// reads each handler's signature and records the schema of any `Json<T>`
/// request body or response into the route's `HttpRouteMeta`, so an OpenAPI
/// generator can describe the payloads with no extra annotation. `T` must
/// implement `nestrs_http::schemars::JsonSchema` (handlers returning a raw
/// `Response`/`String` carry no schema and need no such bound).
///
/// Emits two impls on the controller:
/// - `nestrs_http::Controller` — the mount entry point used by the HTTP
///   transport.
/// - `nestrs_core::Discoverable` — attaches an `HttpControllerMeta` that
///   carries the declarative route table (verb + path + handler name) plus
///   a closure capturing the typed mount logic. The transport iterates
///   these metas at boot.
#[proc_macro_attribute]
pub fn routes(args: TokenStream, input: TokenStream) -> TokenStream {
    routes::routes(args, input)
}

/// Generate a controller's standard REST operations (list/get/create/update/
/// delete) on a `#[controller]`-marked struct's impl block, re-emitting it under
/// `#[routes]`. See the crate docs for the grammar: `#[crud(entity = …::Entity,
/// output = Dto, create = CreateDto, update = UpdateDto, readonly,
/// paginate = cursor|page)]`.
///
/// Each generated handler is thin: `Repo<E>` scopes reads to the caller's ability
/// and joins the request transaction, `Bind<E, A>` loads + instance-checks by id,
/// and `Authorize<Action, E>` gates the route and installs the ambient ability.
/// Guards are **not** a `#[crud]` option — declare them once on the controller
/// (`#[use_guards(...)]` on the struct) and every generated route inherits them.
/// Write a `list`/`get`/`create`/`update`/`delete` method yourself to override
/// that operation — the macro keeps it and skips generating its own.
#[proc_macro_attribute]
pub fn crud(args: TokenStream, input: TokenStream) -> TokenStream {
    crud::entry(args, input)
}
