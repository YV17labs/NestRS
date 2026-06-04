//! GraphQL support, mirroring HTTP's `#[controller]`/`#[routes]` model.
//! `#[resolver]` builds from the container and registers `#[query]` /
//! `#[mutation]` in a link-time [`inventory`]. The schema composes itself at
//! boot — there is no central `queries = [...]` list. Import [`GraphqlModule`]
//! to serve it over HTTP.
//!
//! The roots merge fields from the registry at runtime (not a compile-time
//! `MergedObject` tuple) — the bridge to async-graphql's static
//! `Schema<Q, M, S>`.

mod config;
mod context;
mod guard;
mod loader;
mod module;
mod resolver;

pub use config::GraphqlConfig;
pub use context::ContextSeed;
/// Per-operation seam the endpoint runs around every request. Implemented by
/// `nestrs_authz::graphql`, bound with
/// `providers = [MyBridge as dyn OperationGuard]`.
pub use context::{BoxFuture, OperationGuard};
pub use guard::ResolverGuard;
/// Re-establishes per-request ambient state inside a DataLoader batch (the
/// batch runs on a spawned task where request task-locals are gone).
/// Implemented by `nestrs_database::graphql::LoaderScope`.
pub use loader::{BatchContext, BatchFuture, BatchSpawner};
pub use loader::{LoaderRegistration, batch_spawner};
pub use module::{GraphqlModule, GraphqlSetup};
pub use resolver::{ResolverKind, ResolverObject, ResolverRegistration};

pub use async_graphql;
pub use async_graphql_poem;
pub use async_trait::async_trait;
pub use inventory;

pub use nestrs_graphql_macros::{crud, dataloader, resolver};
