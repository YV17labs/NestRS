//! GraphQL decorator macros, re-exported by `nest-rs-graphql`: `#[resolver]` on
//! the struct (construction), `#[operations]` on its impl (the operations).
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod crud;
mod dataloader;
mod resolver;

/// Declares a GraphQL resolver struct; its operations go under
/// [`macro@operations`] on the impl block.
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
/// `#[use_guards(...)]` belongs on the struct beside `#[resolver]`, not here.
///
/// **Every `#[query]`/`#[mutation]`/`#[subscription]` declares its access
/// posture** — forgetting one is a compile error, never a silently ungated
/// operation:
///
/// - `#[authorize(Action, Entity)]` — the macro emits the class-level gate
///   (`nest_rs_authz::graphql::authorize`) before the call and automatic
///   response masking (`masked_value_for`) after it. The mask sees through the
///   wire DTO itself, `Option<…>` and `Vec<…>`; scalars pass through; an
///   irreconcilable value fails **closed**. Append `unmasked`
///   (`#[authorize(Read, E, unmasked)]`) to keep the gate but mask a custom
///   shape (e.g. a cursor connection) yourself via
///   `nest_rs_authz::masked_output_ambient`.
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
/// **One `#[ComplexObject]` per wire type** (an async-graphql limit): a
/// `#[field_resolver]` and an auto-resolved `#[expose]`d relation on the same
/// entity collide as `E0119` deep in the expansion. Pick one source per type.
///
/// # Expands to
///
/// `#[query]`/`#[mutation]` methods split into hidden
/// `__<Base>Query` / `__<Base>Mutation` `#[Object]` roots (each registered with
/// the link-time registry), `#[field_resolver]`
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

/// Generates the CRUD operations of a resolver; it stands in for
/// `#[operations]`, never beside it. Operation names derive from the output type
/// (`User` → `users`/`user`/`create_user`/…).
///
/// `#[crud(service = svc, entity = …::Entity, output = Dto, create = CreateDto,
/// update = UpdateDto, ops = [list, get, ...], paginate = cursor|none)]`, where
/// `service` names the injected `CrudService` field. Write a matching
/// operation method to override it — the macro keeps yours and skips its own.
///
/// `ops` selects which operations to generate (omit for all five). A `create`/
/// `update` op needs its input type and the service's `Creatable`/`Updatable`
/// impl; `delete` needs `Deletable`. Requesting an op without its type is a
/// compile error.
///
/// The generated list query is **keyset-paginated by default**
/// (`first: Int, after: ID` — `after` is the previous page's last `id`,
/// UUID-v7 keys being time-ordered); `paginate = none` opts out into the
/// full collection, backstopped by `CrudService::list`'s hard cap.
///
/// # Expands to
///
/// The missing operation methods — each delegating to the entity's
/// `CrudService` and declaring `#[authorize(Action, Entity)]` — prepended to the
/// impl block, then the whole block re-emitted under `#[operations]`.
#[proc_macro_attribute]
pub fn crud(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(crud::entry(args, input).into()).into()
}

/// Each method `async fn name(&self, keys: &[K]) -> HashMap<K, V>` (or
/// `Result<HashMap<K, V>, E>`) generates a hidden `Loader` named
/// `<Owner><Name>` and registers it with the link-time registry — no
/// `#[module(providers = [...])]` entry. The loader is
/// **request-scoped**, seeded into the GraphQL context and read by a
/// `#[field_resolver]` as `&DataLoader<…>`.
///
/// # Generated loader names
///
/// The loader type is named **`{Owner}{PascalMethod}`** — the owning struct's
/// name followed by the method name in PascalCase.
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
/// `Loader<K>`, plus a link-time registration that builds the request's
/// `DataLoader` from the assembled container. The loader's `Error` is the method's `E`, or
/// `std::convert::Infallible` when the method returns a bare `HashMap`.
#[proc_macro_attribute]
pub fn dataloader(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(dataloader::dataloader(args, input).into()).into()
}
