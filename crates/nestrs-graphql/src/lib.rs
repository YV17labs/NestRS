//! GraphQL support for nestrs, mirroring the HTTP `#[controller]`/`#[routes]`
//! model.
//!
//! - **Per-resolver** — `#[resolver]` on a struct builds it from the container
//!   (`#[inject]` fields). `#[resolver]` on its impl block splits the
//!   `#[query]`/`#[mutation]` methods into generated `#[Object]` roots and
//!   registers each in a link-time [`inventory`] registry.
//! - **Composition** — the schema composes itself at boot from that registry;
//!   there is no central `queries = [...]` list. Import [`GraphqlModule`] in a
//!   `#[module]` to serve it over HTTP.
//!
//! The roots merge their fields from the registry at runtime rather than from a
//! compile-time `MergedObject` tuple, which is what keeps this compatible with
//! async-graphql's static `Schema<Q, M, S>`.

mod loader;
mod module;
mod resolver;

pub use module::{GraphqlModule, GraphqlOptions, GraphqlSetup};
// `pub` only so `#[resolver]`/`#[dataloader]`-generated code can name them;
// `#[doc(hidden)]` at their definitions keeps them out of the app-facing surface.
pub use loader::LoaderRegistration;
pub use resolver::{ResolverKind, ResolverObject, ResolverRegistration};

pub use async_graphql;
pub use async_graphql_poem;
// Re-exported so `#[resolver]`-generated `inventory::submit!` resolves through
// the framework — apps never depend on `inventory` directly.
pub use inventory;

/// GraphQL decorators (`#[resolver]`, `#[dataloader]`), defined in
/// `nestrs-graphql-macros` and surfaced here so apps write
/// `nestrs_graphql::resolver`.
pub use nestrs_graphql_macros::{dataloader, resolver};
