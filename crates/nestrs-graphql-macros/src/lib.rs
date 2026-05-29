//! GraphQL decorator macros, re-exported by `nestrs-graphql`. The generated
//! code uses absolute paths (`::nestrs_graphql::*`, `::std::sync::Arc`), so this
//! crate does not depend on them — they resolve at the call site.
//!
//! Mirrors the HTTP `#[controller]`/`#[routes]` split: `#[resolver]` on a struct
//! handles construction (DI); `#[resolver]` on its impl block orchestrates the
//! method-level `#[query]`/`#[mutation]`/`#[field]` verbs.
//!
//! `#[field]` is the field-resolver verb (NestJS's `@ResolveField`): it adds a
//! computed/related field to an object type. Its parameters mirror NestJS's
//! `@Parent`/`@Args`/`@Loader`: the first, `parent: &ParentType`, is the
//! resolved object; owned parameters are GraphQL arguments; `&`-reference
//! parameters are injected dependencies — a `&Service` from the container, a
//! request-scoped `&DataLoader<…>` from the request context.
//! It lowers to async-graphql's `#[ComplexObject]`; see `resolver::field_method`.
//!
//! The `#[proc_macro_attribute]` entry functions live here (the language forces
//! them to the crate root); each is a thin delegation to its implementation
//! module (`resolver`, `crud`, `dataloader`).

use proc_macro::TokenStream;

mod crud;
mod dataloader;
mod resolver;

/// Mark a GraphQL resolver.
///
/// Applied in two places, like `#[controller]` + `#[routes]`:
///
/// - **On the struct** — builds it from the container (`#[inject]` fields
///   resolved, others default), emitting `from_container`. The resolver is not
///   a provider; it is constructed at schema-build time.
/// - **On its impl block** — each method tagged `#[query]` or `#[mutation]` is
///   split into a generated `#[Object]` root (`__<Name>Query` /
///   `__<Name>Mutation`) that delegates back to the inherent method, and is
///   submitted to the link-time registry. The schema composes itself from that
///   registry (see `nestrs_graphql::build_schema`) — there is no central list.
///   Each method tagged `#[field]` instead becomes a field resolver on its
///   `parent: &ParentType` argument's type, emitted as a `#[ComplexObject]`
///   impl that delegates to the inherent method (see the module docs).
///
/// **Guards.** A `#[use_guards(GuardA, GuardB)]` on the impl block runs those
/// [`ResolverGuard`](crate::ResolverGuard)s before *every* operation (the
/// `@UseGuards` on a `@Resolver` class analog); one on an individual
/// `#[query]`/`#[mutation]`/`#[field]` guards just that operation and stacks
/// inside the resolver-level ones. Each is resolved from the container and run
/// with the operation's context; a denial short-circuits as a GraphQL error, so a
/// guarded operation returns an `async_graphql::Result<_>`.
#[proc_macro_attribute]
pub fn resolver(args: TokenStream, input: TokenStream) -> TokenStream {
    resolver::resolver(args, input)
}

/// Generate a resolver's standard GraphQL operations (list/get/create/update/
/// delete) on a `#[resolver]`-shaped impl block, re-emitting it under
/// `#[resolver]`. Operation names derive from the output type (`User` →
/// `users`/`user`/`create_user`/…). See [`crud`](crud::crud) and the crate docs:
/// `#[crud(entity = …::Entity, output = Dto, create = CreateDto, update = UpdateDto,
/// readonly)]`.
///
/// Each generated resolver is thin: `Repo<E>` scopes reads to the caller's
/// ability and joins the request transaction, `bind` loads + instance-checks by
/// id, and `authorize::<Action, E>` gates the operation. Write a matching
/// operation method yourself to override it — the macro keeps it and skips its own.
#[proc_macro_attribute]
pub fn crud(args: TokenStream, input: TokenStream) -> TokenStream {
    crud::entry(args, input)
}

/// Turn a data-layer impl block into batched DataLoaders — one per method.
///
/// Each method `async fn name(&self, keys: &[K]) -> HashMap<K, V>` (or
/// `Result<HashMap<K, V>, E>`) generates a hidden `Loader` named `<Owner><Name>`
/// (e.g. `UsersServiceByName`) wrapping `Arc<Owner>` and delegating to the
/// method, and submits a `LoaderRegistration` to the link-time registry — no
/// `#[module(providers = [...])]` entry. The loader is **request-scoped**: a
/// schema extension (installed by `GraphqlModule`) rebuilds it from the fully
/// assembled container at the start of each request and seeds it into the
/// GraphQL context, where a `#[field]` reads it as `&DataLoader<UsersServiceByName>`.
/// Concurrent field resolutions within one request collapse into a single
/// `load`, killing the N+1; the per-request instance keeps requests isolated and
/// makes `GraphqlModule`'s import order irrelevant (the loader is built when a
/// request arrives, never at registration time).
///
/// The batch query lives where the future ORM query will: on the service. The
/// spawner is `tokio::spawn`; nestrs apps already run on Tokio.
#[proc_macro_attribute]
pub fn dataloader(args: TokenStream, input: TokenStream) -> TokenStream {
    dataloader::dataloader(args, input)
}
