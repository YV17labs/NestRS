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
/// `#[use_guards(...)]` here runs on every field of the impl, a field
/// resolver's included; `#[use_interceptors(...)]` and
/// `#[use_exception_filters(...)]` here wrap every root field. A method's own
/// stack inside them. A denial short-circuits as a GraphQL error. A
/// `#[meta(EXPR)]` here is declared for every root field, under a field's own
/// value of the same type.
///
/// # Expands to
///
/// The original struct, a hidden constructor and the hidden helpers
/// `#[operations]` reads the struct's injected keys and layers back through,
/// and a resolver-membership descriptor: a resolver listed in no
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
/// The resolver's own `#[use_guards]`, `#[use_interceptors]` and
/// `#[use_exception_filters]` belong on the struct beside `#[resolver]`, not
/// here; a method binds its own beside its role.
///
/// Two parameters are no argument of the schema: the field's tail resolves
/// them once the guards and the gate passed. They are read by their type's
/// last segment, since no value tells a macro what to leave out of
/// async-graphql's argument list, and each through a function returning core's
/// type, so a namesake does not compile: `Scoped<T>` (`nest_rs_core::Scoped`), a
/// provider from the request's scope, and `Ambient<T>` (`nest_rs_core::Ambient`), what a guard attached
/// — a miss of either answers the opaque internal error, the type filed at
/// `error`.
///
/// A `#[field_resolver]`'s `&DataLoader<L>` parameter, read the same way, is the
/// request's loader. It declares the need, as the method itself does for its
/// resolver: the schema build refuses a served field whose resolver, or whose
/// loader's owner, the app does not reach, naming the module to import.
///
/// **Every `#[query]`/`#[mutation]`/`#[subscription]` declares its access
/// posture** — forgetting one is a compile error, never a silently ungated
/// operation:
///
/// - `#[authorize(Action, Entity)]` — the macro emits the class gate
///   (`nest_rs_authz::class_gate`, run after the operation's guards) before the
///   call and automatic response masking (`masked_value_for`) after it. The mask sees through the
///   wire DTO itself, `Option<…>` and `Vec<…>`; scalars pass through; an
///   irreconcilable value fails **closed**. Append `unmasked`
///   (`#[authorize(Read, E, unmasked)]`) to keep the gate but mask a custom
///   shape (e.g. a cursor connection) yourself via
///   `nest_rs_authz::masked_output_ambient`.
/// - `#[public]` — a caller with no credential is admitted: no `#[authorize]`
///   gate, no response mask. Struct- and method-level `#[use_guards]` still run.
///
/// A `#[field_resolver]` takes neither: it inherits the operation's posture
/// (bind `#[use_guards]` beside it for an extra per-field gate).
///
/// `#[meta(EXPR)]` (repeatable) on a root field declares a value for it, read
/// over the resolver's of the same type; a `#[field_resolver]` declares none.
///
/// **A `#[subscription]` declares the same posture and it is enforced twice**:
/// its layers and gate once, at subscribe, and the mask on **every item** the
/// stream yields — evaluated against the ability the gate checked, captured at
/// subscribe, so an item the subscriber may not read is dropped rather than
/// nulled (`nest_rs_authz::graphql::masked_item_for`). The method is a `fn` or an
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
/// `__<Base>Query` / `__<Base>Mutation` `#[Object]` roots and `#[subscription]`
/// methods into a hidden `__<Base>Subscription` root (each registered with the
/// link-time registry), `#[field_resolver]` methods merge into one
/// `#[ComplexObject]` impl per parent type; each field declares its site,
/// submitted so the boot composes every one before any operation runs; plus an
/// `impl Discoverable` (with a no-op `register`) whose keys put each layer, each
/// `Scoped<T>` parameter's provider and each `bind = Service` inside the access
/// contract.
///
/// Each delegating method runs its field's pipeline, in order: the guards (the
/// app's pool on a root field, the resolver's and the method's), the posture's
/// class gate (`nest_rs_authz::class_gate`), the exception filters and
/// interceptors (on a root field the app's, the modules' `#[interceptor]`s and
/// the resolver's, then the method's; on a field resolver its own alone), the
/// per-argument pipes, the inherent method, then the response mask
/// (`nest_rs_authz::graphql::masked_value_for`) over what it returned.
#[proc_macro_attribute]
pub fn operations(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::__private::reroot_impl_half(args.into(), |args| {
        resolver::operations(args.into(), input).into()
    })
    .into()
}

/// Generates the CRUD operations of a resolver; it stands in for
/// `#[operations]`, never beside it. Operation names derive from the
/// resolver's resource, its name without `Resolver`, made singular
/// (`UsersResolver` → `users`/`user`/`create_user`/…).
///
/// `#[crud(service = UsersService, ops = [list, get, ...], paginate =
/// cursor|none)]`, where `service` names the entity's `CrudService`, which the
/// resolver injects. Every type an operation names is read off that service —
/// the entity, the create and update inputs (`Creatable::Create`,
/// `Updatable::Update`) — and the entity's `#[expose]`d wire type
/// (`Exposed::Wire`); each operation reaches the service through the
/// resolver's `#[inject]` field of its type, so a service the resolver does
/// not inject is refused once, at the block. Write a matching operation
/// method to override it — the macro keeps yours and skips its own, and
/// refuses a `paginate` beside your list, which nothing generated reads.
///
/// `ops` selects which operations to generate (omit for all five). A write op
/// reads the service's trait — `create` `Creatable`, `update` `Updatable`,
/// `delete` `Deletable` — so one the service does not offer is refused naming
/// the trait to implement or the op to drop.
///
/// The generated list query is **keyset-paginated by default**
/// (`first: Int, after: ID` — `after` is the previous page's last `id`,
/// UUID-v7 keys being time-ordered); `paginate = none` opts out into the
/// full collection, backstopped by `CrudService::list`'s hard cap.
///
/// `api(…)`, which documents a controller's generated routes, is refused: an
/// operation that needs a description is one you write, with its doc comment.
///
/// # Expands to
///
/// A hidden alias per type the operations name, read off the service, and a
/// hidden function reaching it through the resolver's field; the
/// missing operation methods — each delegating to the entity's `CrudService`
/// and declaring `#[authorize(Action, Entity)]` — prepended to the impl block,
/// then the whole block re-emitted under `#[operations]`.
#[proc_macro_attribute]
pub fn crud(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::__private::reroot_as("`#[crud]`", crud::entry(args, input).into()).into()
}

/// Each method `async fn name(&self, keys: &[K]) -> HashMap<K, V>` (or
/// `Result<HashMap<K, V>, E>`) generates a hidden `Loader` named
/// `<Owner><Name>` and registers it with the link-time registry — no
/// `#[module(providers = [...])]` entry. The loader is
/// **request-scoped**, seeded into the GraphQL context and read by a
/// `#[field_resolver]` through a `&DataLoader<…>` parameter, which also declares
/// the need: the schema build refuses a served field whose loader's owner the
/// app does not reach, naming the module to import.
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
/// Name the loader exactly that from the `#[field_resolver]` that reads it:
/// `author: &DataLoader<UsersServiceById>`.
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
