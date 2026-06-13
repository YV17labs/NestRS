//! GraphQL decorator macros, re-exported by `nestrs-graphql`. Generated code
//! uses absolute paths, so this crate does not depend on the surface crate.
//!
//! Mirrors the HTTP `#[controller]`/`#[routes]` split: `#[resolver]` on a
//! struct = construction (DI); on its impl =
//! `#[query]`/`#[mutation]`/`#[field_resolver]` orchestration.

use proc_macro::TokenStream;

mod crud;
mod dataloader;
mod resolver;

/// Mark a GraphQL resolver. On the struct: construction via the container
/// (`from_container`). On its impl: `#[query]`/`#[mutation]` methods split
/// into generated `#[Object]` roots and submitted to the link-time registry;
/// `#[field_resolver]` methods become `#[ComplexObject]` impls on the parent type.
///
/// `#[use_guards(...)]` on the impl block runs before every operation;
/// per-method `#[use_guards(...)]` stacks inside it. A denial short-circuits
/// as a GraphQL error.
///
/// **Every `#[query]`/`#[mutation]` declares its access posture** — forgetting
/// one is a compile error, never a silently ungated operation:
///
/// - `#[authorize(Action, Entity)]` — the GraphQL analog of the HTTP
///   `Authorize<A, E>` extractor: the macro emits the class-level gate
///   (`nest_rs_authz::graphql::authorize`) before the call and automatic
///   response masking (`masked_value_for`) after it. The mask sees through the
///   wire DTO itself, `Option<…>` and `Vec<…>`; scalars pass through; an
///   irreconcilable value fails **closed**. Append `unmasked`
///   (`#[authorize(Read, E, unmasked)]`) to keep the gate but mask a custom
///   shape (e.g. a cursor connection) yourself via `masked_output_for`.
///   Requires a `Result` return so denials can surface.
/// - `#[public]` — deliberately ungated: no `#[authorize]` gate, no response
///   mask. Struct- and method-level `#[use_guards]` still run.
///
/// A `#[field_resolver]` takes neither: it inherits the operation's posture
/// (bind `#[use_guards]` beside it for an extra per-field gate).
///
/// **One `#[ComplexObject]` per wire type.** async-graphql allows at most one
/// `#[ComplexObject]` impl per output type. A `#[field_resolver]` here and an
/// auto-resolved `#[expose]`d relation on the *same* entity both emit one, so
/// they collide — the compiler reports a coherence error (`E0119`) deep in the
/// expansion, not a friendly message. Pick a single source per type: either let
/// the relation auto-resolve, or drop `#[expose]` on that relation and write the
/// field yourself.
///
/// # Expands to
///
/// On the struct: the original struct, a `from_container` constructor, the
/// `__nestrs_injected` / `__nestrs_resolver_guard_specs` helpers `#[resolver]
/// impl` reads back, and a resolver-membership descriptor.
///
/// ```ignore
/// // struct form
/// pub struct UsersResolver { /* … */ }
/// impl UsersResolver {
///     fn from_container(c: &::nest_rs_core::Container) -> Self { /* … */ }
///     pub fn __nestrs_injected() -> Vec<TypeId> { /* inject keys + guards */ }
///     pub fn __nestrs_resolver_guard_specs() -> Vec<ScopedGuardSpec> { /* … */ }
/// }
/// ::nest_rs_core::inventory::submit! { ::nest_rs_core::ResolverDescriptor { … } }
/// ```
///
/// On the impl: `#[query]`/`#[mutation]` methods split into hidden
/// `__<Base>Query` / `__<Base>Mutation` `#[Object]` roots (each submitting a
/// `GraphqlResolverRegistration` to the link-time registry), `#[field_resolver]`
/// methods merge into one `#[ComplexObject]` impl per parent type, plus an
/// `impl Discoverable` (with a no-op `register`).
///
/// Each delegating method wraps the inherent one in (innermost→outermost) the
/// declared posture, then the layered guard chain:
///
/// ```ignore
/// // one #[authorize(Read, users::Entity)] query, inside __UsersResolverQuery
/// async fn user(&self, ctx: &Context<'_>, id: String) -> Result<Option<User>> {
///     /* layered guard chain (global + resolver-scope + method guards) */
///     ::nest_rs_authz::graphql::authorize::<Read, users::Entity>(ctx)?;   // class gate
///     match self.0.user(ctx, id).await {                                  // inherent body
///         Ok(out) => Ok(::nest_rs_authz::graphql::masked_value_for::<
///             Read, users::Entity, _>(ctx, out)?),                        // response mask
///         Err(err) => Err(err),
///     }
/// }
/// ```
///
/// ```ignore
/// // impl form
/// pub struct __UsersResolverQuery(Arc<UsersResolver>);
/// #[::nest_rs_graphql::async_graphql::Object]
/// impl __UsersResolverQuery { /* delegating query methods */ }
/// ::nest_rs_graphql::inventory::submit! { ::nest_rs_graphql::GraphqlResolverRegistration { … } }
///
/// #[::nest_rs_graphql::async_graphql::ComplexObject]
/// impl User { /* #[field_resolver] methods for parent `User` */ }
///
/// impl ::nest_rs_core::Discoverable for UsersResolver { /* injected + no-op register */ }
/// ```
#[proc_macro_attribute]
pub fn resolver(args: TokenStream, input: TokenStream) -> TokenStream {
    resolver::resolver(args, input)
}

/// Generate a resolver's standard CRUD operations on a `#[resolver]`-shaped
/// impl block. Operation names derive from the output type (`User` →
/// `users`/`user`/`create_user`/…).
///
/// `#[crud(entity = …::Entity, output = Dto, create = CreateDto, update =
/// UpdateDto, readonly, paginate = cursor|none)]`. Write a matching operation
/// method to override it — the macro keeps yours and skips its own.
///
/// The generated list query is **keyset-paginated by default**
/// (`first: Int, after: ID` — `after` is the previous page's last `id`,
/// UUID-v7 keys being time-ordered); `paginate = none` opts out into the
/// full collection, backstopped by `CrudService::list`'s hard cap.
///
/// # Expands to
///
/// The missing operation methods — each delegating to the entity's
/// `CrudService` and declaring its posture with `#[authorize(Action, Entity)]`
/// exactly as a hand-written operation would (gate + response mask come from
/// `#[resolver]`'s posture expansion, one mechanism for both) — prepended to
/// the impl block, then the whole block re-emitted under `#[resolver]`.
///
/// ```ignore
/// #[::nest_rs_graphql::resolver]
/// impl UsersResolver {
///     #[query]    #[authorize(Read, Entity)]   async fn users(&self, first: Option<u64>, after: Option<String>) -> Result<Vec<User>> { /* CrudService::page */ }
///     #[query]    #[authorize(Read, Entity)]   async fn user(&self, id) -> Result<Option<User>> { /* CrudService::access */ }
///     #[mutation] #[authorize(Create, Entity)] async fn create_user(&self, input) -> Result<User> { /* … */ }
///     #[mutation] #[authorize(Update, Entity)] async fn update_user(&self, id, input) -> Result<Option<User>> { /* … */ }
///     #[mutation] #[authorize(Delete, Entity)] async fn delete_user(&self, id) -> Result<bool> { /* … */ }
///     // …any hand-written methods kept as-is…
/// }
/// ```
#[proc_macro_attribute]
pub fn crud(args: TokenStream, input: TokenStream) -> TokenStream {
    crud::entry(args, input)
}

/// Turn a data-layer impl block into batched DataLoaders — one per method.
///
/// Each method `async fn name(&self, keys: &[K]) -> HashMap<K, V>` (or
/// `Result<HashMap<K, V>, E>`) generates a hidden `Loader` named
/// `<Owner><Name>` and submits a `GraphqlLoaderRegistration` to the link-time
/// registry — no `#[module(providers = [...])]` entry. The loader is
/// **request-scoped**: rebuilt per request from the fully assembled container
/// (so import order is irrelevant) and seeded into the GraphQL context, read
/// by a `#[field_resolver]` as `&DataLoader<…>`.
///
/// # Expands to
///
/// Per method, a `<Owner><Name>` newtype implementing async-graphql's
/// `Loader<K>`, plus a `GraphqlLoaderRegistration` submitted to the link-time
/// registry whose `seed` builds the request's `DataLoader` from the assembled
/// container.
///
/// ```ignore
/// pub struct UsersServiceById(Arc<UsersService>);
/// impl ::nest_rs_graphql::async_graphql::dataloader::Loader<Uuid> for UsersServiceById {
///     type Value = User;
///     type Error = …; // E from Result<…, E>, else ::std::convert::Infallible
///     async fn load(&self, keys: &[Uuid]) -> Result<HashMap<Uuid, User>, Self::Error> { /* delegate */ }
/// }
/// ::nest_rs_graphql::inventory::submit! {
///     ::nest_rs_graphql::GraphqlLoaderRegistration { owner_type_id, seed: |c, req| { … } }
/// }
/// ```
#[proc_macro_attribute]
pub fn dataloader(args: TokenStream, input: TokenStream) -> TokenStream {
    dataloader::dataloader(args, input)
}
