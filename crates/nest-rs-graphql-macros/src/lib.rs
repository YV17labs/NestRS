//! GraphQL decorator macros, re-exported by `nest-rs-graphql`. Generated code
//! uses absolute paths, so this crate does not depend on the surface crate.
//!
//! Mirrors the HTTP `#[controller]`/`#[routes]` split: `#[resolver]` on the
//! struct = construction (DI); `#[operations]` on its impl =
//! `#[query]`/`#[mutation]`/`#[subscription]`/`#[field_resolver]`
//! orchestration.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod crud;
mod dataloader;
mod resolver;

/// The operations themselves go under [`macro@operations`] on the impl block —
/// one decorator per item shape, the same split as `#[controller]`/`#[routes]`.
///
/// `#[use_guards(...)]` here runs before every operation on the impl;
/// per-method `#[use_guards(...)]` stacks inside it. A denial short-circuits
/// as a GraphQL error.
///
/// # Expands to
///
/// The original struct, a private `from_container` constructor, the hidden
/// helpers `#[operations]` reads the struct's injected keys and guards back
/// through, and a resolver-membership descriptor: a resolver listed in no
/// reachable module's `providers` is left out of the schema with a boot `warn`.
#[proc_macro_attribute]
pub fn resolver(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(resolver::resolver(args, input).into()).into()
}

/// `#[query]`/`#[mutation]` methods split into generated `#[Object]` roots and
/// `#[subscription]` methods into a generated `#[Subscription]` root, each
/// submitted to the link-time registry; `#[field_resolver]` methods become
/// `#[ComplexObject]` impls on the parent type.
///
/// The GraphQL counterpart of `#[routes]` and `#[messages]` — named for what it
/// collects, because the spec calls a query or a mutation an *operation*.
/// `#[use_guards(...)]` belongs on the struct beside `#[resolver]`, not here.
///
/// **Every `#[query]`/`#[mutation]`/`#[subscription]` declares its access
/// posture** — forgetting one is a compile error, never a silently ungated
/// operation:
///
/// - `#[authorize(Action, Entity)]` — the GraphQL analog of the HTTP
///   `Authorize<A, E>` extractor: the macro emits the class-level gate
///   (`nest_rs_authz::graphql::authorize`) before the call and automatic
///   response masking (`masked_value_for`) after it. The mask sees through the
///   wire DTO itself, `Option<…>` and `Vec<…>`; scalars pass through; an
///   irreconcilable value fails **closed**. Append `unmasked`
///   (`#[authorize(Read, E, unmasked)]`) to keep the gate but mask a custom
///   shape (e.g. a cursor connection) yourself via
///   `nest_rs_authz::masked_output_ambient`.
///   Requires a `Result` return so denials can surface.
/// - `#[public]` — deliberately ungated: no `#[authorize]` gate, no response
///   mask. Struct- and method-level `#[use_guards]` still run.
///
/// A `#[field_resolver]` takes neither: it inherits the operation's posture
/// (bind `#[use_guards]` beside it for an extra per-field gate).
///
/// **A `#[subscription]` declares the same posture and it is enforced twice**:
/// the gate once, at subscribe, and the mask on **every item** the stream
/// yields — evaluated against the ability captured at subscribe, so an item the
/// subscriber may not read is dropped rather than nulled
/// (`nest_rs_authz::graphql::masked_item_for`). The method is a `fn` or an
/// `async fn` returning `impl Stream<Item = T>`, optionally behind a
/// literally-spelled `Result<…>`; an aliased `Result` is a compile error that
/// names itself.
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
/// `#[query]`/`#[mutation]` methods split into hidden
/// `__<Base>Query` / `__<Base>Mutation` `#[Object]` roots (each submitting a
/// `GraphqlResolverRegistration` to the link-time registry), `#[field_resolver]`
/// methods merge into one `#[ComplexObject]` impl per parent type, plus an
/// `impl Discoverable` (with a no-op `register`).
///
/// Each delegating method runs the layered guard chain (global, resolver-scope
/// and method guards), the posture's class gate
/// (`nest_rs_authz::graphql::authorize`), the inherent method, then the response
/// mask (`nest_rs_authz::graphql::masked_value_for`) over what it returned.
#[proc_macro_attribute]
pub fn operations(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(resolver::operations(args, input).into()).into()
}

/// It stands in for `#[operations]`, never beside it. Operation names derive
/// from the output type (`User` → `users`/`user`/`create_user`/…).
///
/// `#[crud(service = svc, entity = …::Entity, output = Dto, create = CreateDto,
/// update = UpdateDto, ops = [list, get, ...], paginate = cursor|none)]`, where
/// `service` names the injected `CrudService` field. Write a matching
/// operation method to override it — the macro keeps yours and skips its own.
///
/// `ops` selects which operations to generate (omit for all five). A `create`/
/// `update` op needs its input type and the service's `Creatable`/`Updatable`
/// impl; `delete` needs `Deletable`. Requesting an op without its type is a
/// compile error — a resource exposes only the operations it actually has.
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
/// `#[operations]`' posture expansion, one mechanism for both) — prepended to
/// the impl block, then the whole block re-emitted under `#[operations]`; the
/// hand-written methods are kept as they are.
#[proc_macro_attribute]
pub fn crud(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(crud::entry(args, input).into()).into()
}

/// Each method `async fn name(&self, keys: &[K]) -> HashMap<K, V>` (or
/// `Result<HashMap<K, V>, E>`) generates a hidden `Loader` named
/// `<Owner><Name>` and submits a `GraphqlLoaderRegistration` to the link-time
/// registry — no `#[module(providers = [...])]` entry. The loader is
/// **request-scoped**: rebuilt per request from the fully assembled container
/// (so import order is irrelevant) and seeded into the GraphQL context, read
/// by a `#[field_resolver]` as `&DataLoader<…>`.
///
/// # Generated loader names
///
/// The loader type is named **`{Owner}{PascalMethod}`** — the owning struct's
/// name followed by the method name in PascalCase. A hand-typed wrong name is
/// already a compile error (the type doesn't exist); this table makes the
/// correct name discoverable so you don't guess. Each generated struct also
/// carries a doc comment stating what it is and which method it came from.
///
/// | Owner struct | `#[dataloader]` method | Generated loader |
/// |---|---|---|
/// | `UsersService` | `by_id` | `UsersServiceById` |
/// | `UsersService` | `by_org_id` | `UsersServiceByOrgId` |
/// | `PostsService` | `author` | `PostsServiceAuthor` |
///
/// Reference the loader by exactly that name from the `#[field_resolver]`
/// (or auto-resolved relation) that reads it:
/// `ctx.data_unchecked::<DataLoader<UsersServiceById>>()`.
///
/// # Expands to
///
/// Per method, a `<Owner><Name>` newtype implementing async-graphql's
/// `Loader<K>`, plus a `GraphqlLoaderRegistration` submitted to the link-time
/// registry whose `seed` builds the request's `DataLoader` from the assembled
/// container. The loader's `Error` is the method's `E`, or
/// `std::convert::Infallible` when the method returns a bare `HashMap`.
#[proc_macro_attribute]
pub fn dataloader(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(dataloader::dataloader(args, input).into()).into()
}
