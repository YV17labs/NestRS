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

mod context;
mod guard;
mod loader;
mod module;
mod resolver;

/// Forward a per-request value from the poem request into the GraphQL context —
/// the bridge a resolver (and GraphQL authorization) reads request-scoped state
/// through. Submit one with `inventory`, or — for the common case of forwarding the
/// authenticated principal — the [`forward_principal!`] macro.
pub use context::ContextSeed;
/// The per-operation seam the endpoint runs around every request (authenticate,
/// then wrap execution with ambient state). Implemented by `nestrs-authz-graphql`,
/// bound with `providers = [MyBridge as dyn OperationGuard]`.
pub use context::{BoxFuture, OperationGuard};
/// The per-resolver guard seam (`#[use_guards]` on a `#[resolver]`), the GraphQL
/// analog of HTTP's per-route/controller guards. See [`guard`].
pub use guard::ResolverGuard;
pub use module::{GraphqlModule, GraphqlOptions, GraphqlSetup};
// `pub` only so `#[resolver]`/`#[dataloader]`-generated code can name them;
// `#[doc(hidden)]` at their definitions keeps them out of the app-facing surface.
pub use loader::{batch_spawner, LoaderRegistration};
/// The seam that re-establishes per-request ambient state (the executor, the
/// authorization ability) inside a DataLoader batch — a batch runs on a spawned
/// task where the request task-locals are gone. Implemented by
/// `nestrs-authz-graphql`, bound with `providers = [MyBridge as dyn BatchContext]`.
pub use loader::{BatchContext, BatchFuture, BatchSpawner};
pub use resolver::{ResolverKind, ResolverObject, ResolverRegistration};

pub use async_graphql;
pub use async_graphql_poem;
// Re-exported so a `ResolverGuard` implementor writes `#[nestrs_graphql::async_trait]`
// without depending on `async-trait` directly.
pub use async_trait::async_trait;
// Re-exported so `#[resolver]`-generated `inventory::submit!` resolves through
// the framework — apps never depend on `inventory` directly.
pub use inventory;

/// GraphQL decorators (`#[resolver]`, `#[crud]`, `#[dataloader]`), defined in
/// `nestrs-graphql-macros` and surfaced here so apps write
/// `nestrs_graphql::resolver`. The [`forward_principal!`] macro is defined in
/// `context` and exported at the crate root by `#[macro_export]`.
pub use nestrs_graphql_macros::{crud, dataloader, resolver};
