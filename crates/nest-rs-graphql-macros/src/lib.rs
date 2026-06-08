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
#[proc_macro_attribute]
pub fn resolver(args: TokenStream, input: TokenStream) -> TokenStream {
    resolver::resolver(args, input)
}

/// Generate a resolver's standard CRUD operations on a `#[resolver]`-shaped
/// impl block. Operation names derive from the output type (`User` →
/// `users`/`user`/`create_user`/…).
///
/// `#[crud(entity = …::Entity, output = Dto, create = CreateDto, update =
/// UpdateDto, readonly)]`. Write a matching operation method to override it —
/// the macro keeps yours and skips its own.
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
#[proc_macro_attribute]
pub fn dataloader(args: TokenStream, input: TokenStream) -> TokenStream {
    dataloader::dataloader(args, input)
}
