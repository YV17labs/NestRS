//! GraphQL support, mirroring HTTP's `#[controller]`/`#[routes]` model.
//! `#[resolver]` builds from the container and registers `#[query]` /
//! `#[mutation]` / `#[subscription]` in a link-time [`inventory`]. The schema
//! composes itself at boot — there is no central `queries = [...]` list. Import
//! [`GraphqlModule`] to serve it over HTTP, subscriptions included: the same
//! path carries `POST` and the graphql-ws socket.
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

#![warn(missing_docs)]

/// This crate's span target — Schema execution, subscriptions, and federation entities.
///
/// Declared by the crate that **owns** the concern, which is not always the only
/// crate emitting on it: a sibling and a `*-macros` expansion read this constant
/// rather than spelling a second one, because a target's one job is to say
/// **where** an event came from. A central table in the kernel would have meant
/// `nest-rs-core` holding a name for a concern it does not know exists.
pub const TARGET: &str = "nest_rs::graphql";

mod config;
mod context;
mod error;
mod federation;
mod loader;
mod module;
mod opaque;
mod operation;
mod resolver;
mod scope;
mod subscription;
pub mod unit;

pub use config::GraphqlConfig;
/// Per-operation seam the endpoint runs around every request. Implemented by
/// `nest_rs_authz::graphql`, bound with
/// `providers = [MyBridge as dyn GraphqlOperationGuard]`.
pub use context::{BoxFuture, FallbackOperationGuard, GraphqlOperationGuard, GraphqlVariablePipe};
pub use context::{GraphqlContextSeed, SeedLifetime};
pub use error::{FIELD_ERRORS_EXTENSION, pipe_error};
/// The gate in front of `_service` / `_entities`, which async-graphql resolves
/// above the merged root — implemented by `nest_rs_guards`, which owns the pool.
pub use federation::{FederationGate, GraphqlFederationGuard};
/// Re-establishes per-request ambient state inside a DataLoader batch (the
/// batch runs on a spawned task where request task-locals are gone).
/// Implemented by `nest_rs_seaorm::graphql::LoaderScope`.
pub use loader::{GraphqlBatchContext, GraphqlBatchFuture, GraphqlBatchSpawner};
pub use loader::{GraphqlLoaderRegistration, batch_spawner};
pub use module::{GraphqlModule, GraphqlSetup};
pub use opaque::Opaque;
/// What a `Guard::check_graphql` is handed: one operation, whichever of the two
/// sites async-graphql exposes it at.
pub use operation::GraphqlOperationContext;
pub use resolver::{
    GraphqlResolverKind, GraphqlResolverObject, GraphqlResolverRegistration, GraphqlRootMember,
    GraphqlSubscriptionObject, ResolverDescriptor,
};
/// Resolver-side accessor for `#[injectable(scope = request)]` providers — the
/// GraphQL mirror of `nest_rs_http::Scoped<T>`. Reachable in resolver bodies,
/// **not** inside `#[dataloader]` batch closures (those run off-task).
pub use scope::Scoped;
// Hidden: the per-item posture seam `#[subscription]` expands into. A resolver
// body never calls it — the posture attribute writes the call, exactly as it
// writes `masked_value_for` on a query.
#[doc(hidden)]
pub use subscription::keep_masked_item;
// Hidden: the composed schema as an `Executor`, so a test can be the subscriber
// graphql-ws would otherwise have to be. See the function's doc.
#[doc(hidden)]
pub use subscription::compose_schema;

pub use async_graphql;
pub use async_graphql_poem;
pub use async_trait::async_trait;
// Hidden: macro plumbing — `#[resolver]`-generated `inventory::submit!`
// resolves through the framework; apps never depend on `inventory` directly.
#[doc(hidden)]
pub use inventory;
// Re-exported so `#[crud]`-generated create/update ops validate their input
// (`::nest_rs_graphql::ValidateProbe`) without the consumer depending on
// nest-rs-pipes directly — the global-validation ("ValidationPipe") path.
pub use nest_rs_pipes::{MaybeValidateFallback, ValidateProbe};

pub use nest_rs_graphql_macros::{crud, dataloader};

/// The operations decorator — `#[resolver]`'s impl-block half, the GraphQL
/// counterpart of `#[routes]`.
pub use nest_rs_graphql_macros::operations;
/// The resolver decorator, for the struct. `#[use_interceptors(...)]` /
/// `#[use_filters(...)]` are **HTTP-only** — the per-operation GraphQL seam is
/// reserved but not invoked, so binding one on a resolver is rejected at compile
/// time instead of silently doing nothing:
///
/// ```compile_fail
/// use nest_rs_graphql::resolver;
///
/// #[resolver]
/// #[use_interceptors(SomeInterceptor)]
/// struct BadResolver;
/// ```
pub use nest_rs_graphql_macros::resolver;
