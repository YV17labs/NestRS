//! GraphQL support, mirroring HTTP's `#[controller]`/`#[routes]` model.
//! `#[resolver]` builds from the container and registers `#[query]` /
//! `#[mutation]` in a link-time [`inventory`]. The schema composes itself at
//! boot — there is no central `queries = [...]` list. Import [`GraphqlModule`]
//! to serve it over HTTP.
//!
//! The roots merge fields from the registry at runtime (not a compile-time
//! `MergedObject` tuple) — the bridge to async-graphql's static
//! `Schema<Q, M, S>`.
//!
//! # Pinned async-graphql version
//!
//! [`resolver`] reads async-graphql's public-but-internal registry API: it
//! spells out an exhaustive `MetaType::Object { .. }` literal and relies on
//! `remove_unused_types` behaviour. The workspace therefore pins the *exact*
//! version (`async-graphql = "=7.2.1"` in the root `Cargo.toml`) and guards it
//! in three layers — a compile-time field canary and the exhaustive literal
//! (both in `resolver.rs`) plus the `tests/integration/sdl_snapshot.rs`
//! snapshot test that catches behavioural drift that still compiles.
//!
//! **Bump procedure** (when raising the pin):
//! 1. bump the `=7.2.x` pin for `async-graphql` **and** `async-graphql-poem`
//!    in the root `Cargo.toml`;
//! 2. fix the compile-time canary in `resolver.rs` (and the matching
//!    `MetaType::Object` literal) until the crate compiles again;
//! 3. run the SDL snapshot test (`cargo nextest run -p nest-rs-graphql`);
//! 4. review the SDL diff — an intended change means updating the committed
//!    snapshot; an unexpected one is a regression in the composed schema.

mod config;
mod context;
mod guard;
mod loader;
mod module;
mod resolver;
mod scope;

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
/// Resolver-side accessor for `#[injectable(scope = request)]` providers — the
/// GraphQL mirror of `nest_rs_http::Scoped<T>`. Reachable in resolver bodies,
/// **not** inside `#[dataloader]` batch closures (those run off-task).
pub use scope::Scoped;

pub use async_graphql;
pub use async_graphql_poem;
pub use async_trait::async_trait;
pub use inventory;
// Re-exported so `#[crud]`-generated create/update ops validate their input
// (`::nest_rs_graphql::ValidateProbe`) without the consumer depending on
// nest-rs-pipes directly — the global-validation ("ValidationPipe") path.
pub use nest_rs_pipes::{MaybeValidateFallback, ValidateProbe};

pub use nest_rs_graphql_macros::{crud, dataloader};

/// The resolver decorator. `#[use_interceptors(...)]` / `#[use_filters(...)]`
/// are **HTTP-only** — the per-operation GraphQL seam is reserved but not
/// invoked, so binding one on a resolver is rejected at compile time instead of
/// silently doing nothing:
///
/// ```compile_fail
/// use nest_rs_graphql::resolver;
///
/// #[resolver]
/// #[use_interceptors(SomeInterceptor)]
/// struct BadResolver;
/// ```
pub use nest_rs_graphql_macros::resolver;
