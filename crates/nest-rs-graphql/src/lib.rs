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
pub use context::GraphqlContextSeed;
/// Per-operation seam the endpoint runs around every request. Implemented by
/// `nest_rs_authz::graphql`, bound with
/// `providers = [MyBridge as dyn GraphqlOperationGuard]`.
pub use context::{BoxFuture, FallbackOperationGuard, GraphqlOperationGuard, GraphqlVariablePipe};
pub use guard::GraphqlResolverGuard;
/// Re-establishes per-request ambient state inside a DataLoader batch (the
/// batch runs on a spawned task where request task-locals are gone).
/// Implemented by `nest_rs_seaorm::graphql::LoaderScope`.
pub use loader::{GraphqlBatchContext, GraphqlBatchFuture, GraphqlBatchSpawner};
pub use loader::{GraphqlLoaderRegistration, batch_spawner};
pub use module::{GraphqlModule, GraphqlSetup};
pub use resolver::{GraphqlResolverKind, GraphqlResolverObject, GraphqlResolverRegistration};

pub use async_graphql;
pub use async_graphql_poem;
pub use async_trait::async_trait;
pub use inventory;
// Re-exported so `#[crud]`-generated create/update ops validate their input
// (`::nest_rs_graphql::ValidateProbe`) without the consumer depending on
// nest-rs-pipes directly — the global-validation ("ValidationPipe") path.
pub use nest_rs_pipes::{MaybeValidateFallback, ValidateProbe};

pub use nest_rs_graphql_macros::{crud, dataloader, resolver};
