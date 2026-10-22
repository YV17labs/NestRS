//! GraphQL support, mirroring HTTP's `#[controller]`/`#[routes]` model.
//! `#[resolver]` builds from the container and registers `#[query]` /
//! `#[mutation]` / `#[subscription]` in a link-time registry; the schema
//! composes itself at boot. Import [`GraphqlModule`] to serve it over HTTP,
//! subscriptions included: the same path carries `POST` and the graphql-ws socket.
//!
//! # Pinned async-graphql version
//!
//! `src/resolver.rs` reads async-graphql's registry internals, so the workspace
//! pins the exact version, held by a compile-time canary in `resolver.rs` and the
//! `tests/integration/sdl_snapshot.rs` snapshot. To bump it: raise the pin, fix
//! the canary until the crate compiles, then review the SDL snapshot diff.
//! `src/null_propagation.rs` reads three of its behaviours — a failed field's
//! key left out, the first of a field's selections to finish kept, a request's
//! cached parse executed — each held by its tests.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — schema execution, subscriptions, and federation entities.
pub const TARGET: &str = "nest_rs::graphql";

mod config;
mod context;
mod endpoint;
mod error;
mod federation;
mod loader;
mod module;
mod null_propagation;
mod opaque;
mod operation;
mod operation_units;
mod redact;
mod resolver;
mod scope;
mod subscription;
pub mod unit;
mod view;

pub use config::GraphqlConfig;
pub use context::BoxFuture;
pub use context::{GraphqlContextSeed, SeedLifetime};
pub use error::{
    CODE_EXTENSION, FIELD_ERRORS_EXTENSION, denial_to_graphql_error, pipe_error, problem_error,
};
pub use module::{GraphqlModule, GraphqlSetup};
pub use opaque::Opaque;
pub use scope::Scoped;
pub use view::{FieldRole, GraphqlView};

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::context::{ENDPOINT, connection_data};
    pub use crate::error::{ErrorReport, ErrorReportChain, ErrorReportDeliberate, FieldFailure};
    pub use crate::loader::{GraphqlLoaderRegistration, batch_spawner};
    pub use crate::operation::{
        FieldGuards, GraphqlSite, IsStreamReturn, ROOT_FIELD, answers_a_stream, field_guard_stage,
        field_name, run_field, run_nested,
    };
    pub use crate::resolver::{
        GraphqlResolverKind, GraphqlResolverObject, GraphqlResolverRegistration, GraphqlRootMember,
        GraphqlSiteRegistration, GraphqlSubscriptionObject, ResolverDescriptor,
    };
    pub use crate::scope::{FromScope, unprovided};
    pub use crate::subscription::{compose_schema, keep_masked_item};

    pub use inventory;
    pub use nest_rs_core::ambient;
    // For `#[crud]`-generated ops, so the consumer needs no nest-rs-pipes dependency.
    pub use nest_rs_pipes::__private::{MaybeValidateFallback, ValidateProbe};
}

pub use async_graphql;
pub use async_trait::async_trait;

/// Generate a resolver's standard CRUD operations on its impl block, in place
/// of [`operations`]; each one delegates to the entity's `CrudService` and
/// declares `#[authorize(Action, Entity)]`, as a hand-written one would.
pub use nest_rs_graphql_macros::crud;

/// Turn a data-layer impl block into batched DataLoaders, one per method, each
/// named `{Owner}{PascalMethod}`.
///
/// ```
/// use std::collections::HashMap;
/// use std::convert::Infallible;
/// use nest_rs_core::injectable;
/// use nest_rs_graphql::async_graphql::dataloader::Loader;
/// use nest_rs_graphql::dataloader;
///
/// #[injectable]
/// #[derive(Default)]
/// struct UsersService;
///
/// #[dataloader]
/// impl UsersService {
///     async fn by_id(&self, keys: &[i32]) -> HashMap<i32, String> {
///         keys.iter().map(|id| (*id, format!("user {id}"))).collect()
///     }
/// }
///
/// fn implements<T: Loader<i32, Value = String, Error = Infallible>>() {}
/// implements::<UsersServiceById>();
/// ```
pub use nest_rs_graphql_macros::dataloader;

/// Declare a [`resolver`](macro@resolver)'s operations on its impl block. Each operation
/// declares its posture, `#[public]` or `#[authorize(Action, Entity)]`; the
/// gate and the reply mask `#[authorize]` emits run in `nest_rs_authz::graphql`.
///
/// ```
/// use nest_rs_core::Discoverable;
/// use nest_rs_graphql::async_graphql::{Result, SimpleObject};
/// use nest_rs_graphql::{operations, resolver};
/// # use nest_rs_core::module;
/// # use nest_rs_graphql::GraphqlModule;
///
/// #[derive(SimpleObject)]
/// struct User {
///     id: i32,
///     name: String,
/// }
///
/// #[resolver]
/// struct UsersResolver;
///
/// #[operations]
/// impl UsersResolver {
///     #[query]
///     #[public]
///     async fn user(&self, id: i32) -> Result<Option<User>> {
///         Ok(Some(User { id, name: "Ada".into() }))
///     }
/// }
///
/// fn implements<T: Discoverable>() {}
/// # #[module(imports = [GraphqlModule::for_root(None)], providers = [UsersResolver])]
/// # struct AppModule;
/// #
/// # async fn query(app: &nest_rs_testing::TestApp, query: &str) -> serde_json::Value {
/// #     let resp = app.http().post("/graphql").body_json(&serde_json::json!({ "query": query })).send().await;
/// #     resp.json().await.value().deserialize()
/// # }
/// #
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// implements::<UsersResolver>();
/// # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
///
/// let reply = query(&app, "{ user(id: 1) { name } }").await;
/// assert_eq!(reply["data"]["user"]["name"], "Ada");
/// # Ok(())
/// # }
/// ```
pub use nest_rs_graphql_macros::operations;
/// Mark a GraphQL resolver struct: built from the container, its
/// `#[use_guards(...)]`, `#[use_interceptors(...)]` and
/// `#[use_exception_filters(...)]` the resolver-scope layers.
///
/// ```
/// use std::sync::Arc;
/// use nest_rs_core::{injectable, module};
/// use nest_rs_graphql::{GraphqlModule, operations, resolver};
///
/// #[injectable]
/// #[derive(Default)]
/// struct GreetingService;
///
/// impl GreetingService {
///     fn greet(&self) -> String {
///         "hello".into()
///     }
/// }
///
/// #[resolver]
/// struct GreetingResolver {
///     #[inject]
///     svc: Arc<GreetingService>,
/// }
///
/// #[operations]
/// impl GreetingResolver {
///     #[query]
///     #[public]
///     async fn greeting(&self) -> String {
///         self.svc.greet()
///     }
/// }
///
/// #[module(
///     imports = [GraphqlModule::for_root(None)],
///     providers = [GreetingService, GreetingResolver],
/// )]
/// struct AppModule;
/// # #[nest_rs_core::main]
/// # async fn main() -> nest_rs_core::anyhow::Result<()> {
/// # let app = nest_rs_testing::TestApp::for_module::<AppModule>().await?;
///
/// let resp = app
///     .http()
///     .post("/graphql")
///     .body_json(&serde_json::json!({ "query": "{ greeting }" }))
///     .send()
///     .await;
/// resp.assert_json(serde_json::json!({ "data": { "greeting": "hello" } })).await;
/// # Ok(())
/// # }
/// ```
///
/// Its interceptors and exception filters wrap each of its root fields, inside
/// the app's own; a `#[field_resolver]` runs only those declared on it.
/// `#[use_filters(...)]` is refused at compile time: an exception filter
/// catching `dyn Error + Send + Sync` takes its place.
///
/// ```compile_fail
/// use nest_rs_graphql::resolver;
///
/// #[resolver]
/// #[use_filters(SomeFilter)]
/// struct BadResolver;
/// ```
pub use nest_rs_graphql_macros::resolver;
