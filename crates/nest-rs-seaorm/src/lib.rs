//! SeaORM for nestrs — the adapter that wraps `sea_orm` over PostgreSQL, the
//! one engine it supports, as CI runs it. One substrate and its bindings:
//!
//! - [`SeaOrmModule::for_root`] resolves [`SeaOrmConfig`] (`<PREFIX>_SEAORM__*`)
//!   and opens the one `sea_orm::DatabaseConnection` every binding shares,
//!   closing it when the app ends, after every shutdown hook, without waiting
//!   past the settle for a connection no unit holds.
//! - [`SeaOrmDatabaseModule`] (bare) binds the `nest-rs-database` port:
//!   [`SeaOrmUnitOfWork`], which every edge — an HTTP request, a WS message, an
//!   MCP operation, a queue attempt, a scheduled tick — and every lifecycle hook
//!   runs its unit of work through, on an ambient [`Executor`]: the pool, or a
//!   transaction opened on the unit's first data-layer touch.
//! - [`SeaOrmHealthModule`] (bare, feature `health`) binds the health port: the
//!   pool's indicator, `seaorm`, on the readiness and startup probes.
//! - Under the `queue` feature, [`SeaOrmDatabaseModule`] also keeps the queue
//!   port's outbox, [`queue::SeaOrmOutbox`]: a job pushed inside a unit's
//!   transaction is written there and filed after its commit;
//!   [`queue::OutboxMigration`] creates its table.
//!
//! Services query through [`Repo`]: every call runs on the ambient executor,
//! every read is filtered by the caller's [`Ability`](nest_rs_authz::Ability),
//! and an entity declared `#[expose(..., soft_delete)]` keeps its deleted rows
//! out of every read but [`Repo::with_tombstones`].
//! [`nest_rs_database::atomic`] runs a block inside a [`Savepoint`] of that
//! executor's transaction, or a transaction of its own on the pool.
//! An entity crosses the wire through [`macro@expose`], and a column's enum
//! through [`macro@wire_enum`].
//!
//! ```
//! # use nest_rs_core::module;
//! # use nest_rs_seaorm::sea_orm::{ConnectOptions, Database};
//! # use nest_rs_seaorm::{SeaOrmDatabaseModule, SeaOrmModule};
//! # use nest_rs_testing::TestApp;
//! # use nest_rs_database::UnitOfWork;
//! # #[module(providers = [])]
//! # pub struct UsersModule;
//! #[module(imports = [SeaOrmModule::for_root(None), SeaOrmDatabaseModule, UsersModule])]
//! pub struct AppModule;
//! # #[nest_rs_core::main]
//! # async fn main() -> anyhow::Result<()> {
//! # let mut options = ConnectOptions::new("postgres://app@localhost/app");
//! # options.connect_lazy(true);
//! # let db = Database::connect(options).await?;
//! # let app = TestApp::builder().provide(db).module::<AppModule>().build().await?;
//!
//! assert!(app.container().get_dyn::<dyn UnitOfWork>().is_some());
//! # Ok(())
//! # }
//! ```
//!
//! Pin explicit values with [`SeaOrmModule::for_root`]`(SeaOrmConfig { .. })`.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod atomic;
mod config;
mod database;
mod error;
mod executor;
mod module;
mod page;
mod repo;
pub mod retry;
mod savepoint;
mod service;
mod slug;
mod soft_delete;
pub mod target;
mod time;
mod turn;
mod unit;

#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "health")]
mod health;
#[cfg(feature = "http")]
mod http;
#[cfg(feature = "queue")]
pub mod queue;

pub use config::SeaOrmConfig;
pub use database::SeaOrmDatabaseModule;
pub use error::{CommitError, DbOutcome, ServiceError};
pub use executor::{
    Executor, ExecutorScope, FinalizeOutcome, LazyTransaction, current_executor,
    current_executor_scope, with_executor, with_job_executor, with_request_executor,
};
pub use module::{SeaOrmModule, SeaOrmSetup, connect_from_env};
pub use page::{DEFAULT_PAGE_SIZE, LIST_CAP, Page, PageParams, clamp_page_size};
pub use repo::{Repo, scope_for};
pub use savepoint::Savepoint;
pub use service::{
    Access, Authorized, Creatable, CreateModel, CrudService, Deletable, Updatable, UpdateModel,
    model_uuid,
};
pub use slug::resolve_unique_slug;
pub use soft_delete::{SoftDeletable, live_condition};
pub use time::now;
pub use unit::SeaOrmUnitOfWork;

#[cfg(feature = "health")]
pub use health::SeaOrmHealthModule;
#[cfg(feature = "http")]
pub use http::Bind;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::config::DEFAULT_STATEMENT_TIMEOUT;
    #[cfg(feature = "graphql")]
    pub use crate::error::graphql_crud_error;
    #[cfg(feature = "http")]
    pub use crate::error::http_crud_error;
    pub use crate::error::masking;

    pub use crate::soft_delete::{SoftDeleteDeclared, SoftDeleteRegistration};
    pub use inventory;

    // What `#[expose]` and `#[wire_enum]` emit, so an entity crate declares
    // none of these; each derive carries a `crate = ` override pointing here.
    pub use async_trait::async_trait;
    pub use chrono;
    #[cfg(feature = "graphql")]
    pub use nest_rs_graphql::{async_graphql, dataloader};
    pub use schemars;
    pub use serde;
    pub use serde_json;
    pub use tracing;
    pub use uuid;
    pub use validator;
}

/// Expose a SeaORM entity to REST/OpenAPI (and optionally GraphQL) from one
/// declaration: its wire DTO (`Serialize` + `JsonSchema`), its `Create`/`Update`
/// inputs, and the [`WireModelDefaults`](nest_rs_authz::WireModelDefaults) impl
/// response masking reads.
///
/// **Exposure is opt-in:** a column reaches the wire only when its field
/// carries `#[expose]` (or `#[expose(input(...))]`, which implies read). A field
/// with no `#[expose]` stays hidden on every transport, so a column added by a
/// later migration never leaks by omission.
///
/// ```
/// # use nest_rs_authz::WireModelDefaults;
/// # use nest_rs_seaorm::expose;
/// # mod service {
/// #     pub struct UsersService;
/// # }
/// # mod user {
/// #     use super::*;
/// #     use sea_orm::entity::prelude::*;
/// #[expose(name = "User", service = super::service::UsersService)]
/// #[sea_orm::model]
/// # #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
/// # #[sea_orm(table_name = "users")]
/// pub struct Model {
///     #[sea_orm(primary_key, auto_increment = false)]
///     #[expose]                                                  // read-only
///     pub id: Uuid,
///     #[expose]                                                  // read-only
///     pub org_id: Uuid,
///     #[expose(input(create, update), validate(length(min = 1)))] // read + write
///     pub name: String,
///     #[expose(input(create), validate(email))]                  // read + create-only
///     pub email: String,
///     pub password_hash: Option<String>,                         // no #[expose] ⇒ hidden
/// }
/// # impl ActiveModelBehavior for ActiveModel {}
/// # }
/// # fn main() -> Result<(), serde_json::Error> {
/// # use sea_orm::prelude::Uuid;
/// # use user::{CreateUser, Entity, Model, UpdateUser, User};
/// # let id = Uuid::nil();
///
/// let model = Model {
///     id,
///     org_id: id,
///     name: "Ada".into(),
///     email: "ada@example.com".into(),
///     password_hash: Some("$argon2id$…".into()),
/// };
/// assert_eq!(
///     serde_json::to_value(User::from(&model))?,
///     serde_json::json!({ "id": id, "org_id": id, "name": "Ada", "email": "ada@example.com" }),
/// );
/// let _ = CreateUser { name: "Ada".into(), email: "ada@example.com".into() };
/// let _ = UpdateUser { name: "Ada L.".into() };
///
/// fn implements<T: WireModelDefaults>() {}
/// implements::<Entity>();
/// # Ok(())
/// # }
/// ```
///
/// Add the `graphql` flag, with this crate's `graphql` feature, and the DTO is
/// also a GraphQL object, its `#[expose]`d relations resolved through
/// dataloaders ([`graphql::RelatedTo`]); without the flag the entity crate
/// compiles no GraphQL at all:
///
/// ```
/// # use nest_rs_seaorm::expose;
/// # mod service {
/// #     pub struct ItemsService;
/// # }
/// # mod rest {
/// #     use super::*;
/// #     use sea_orm::entity::prelude::*;
/// #[expose(name = "Item", service = super::service::ItemsService)]
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
/// #     #[sea_orm(table_name = "items")]
/// #     pub struct Model {
/// #         #[sea_orm(primary_key)]
/// #         #[expose]
/// #         pub id: i32,
/// #     }
/// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// #     pub enum Relation {}
/// #     impl ActiveModelBehavior for ActiveModel {}
/// # }
/// # mod graph {
/// #     use super::*;
/// #     use sea_orm::entity::prelude::*;
/// #[expose(name = "Item", service = super::service::ItemsService, graphql)]
/// #     #[derive(Clone, Debug, PartialEq, DeriveEntityModel, serde::Serialize)]
/// #     #[sea_orm(table_name = "items")]
/// #     pub struct Model {
/// #         #[sea_orm(primary_key)]
/// #         #[expose]
/// #         pub id: i32,
/// #     }
/// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// #     pub enum Relation {}
/// #     impl ActiveModelBehavior for ActiveModel {}
/// # }
/// # fn main() {
/// # use async_graphql::OutputType;
///
/// fn wire_only<T: serde::Serialize + schemars::JsonSchema>() {}
/// fn with_graphql<T: serde::Serialize + schemars::JsonSchema + OutputType>() {}
/// wire_only::<rest::Item>();
/// with_graphql::<graph::Item>();
/// # }
/// ```
pub use nest_rs_seaorm_macros::expose;

/// The enum mode of [`macro@expose`]: make a column's enum type a wire type.
///
/// A column whose type is a custom enum passes through to the wire verbatim,
/// so the enum itself carries the wire traits, each routed back through this
/// crate so the entity crate declares neither `schemars` nor `async-graphql`.
///
/// ```
/// # use nest_rs_seaorm::wire_enum;
/// # use sea_orm::entity::prelude::*;
/// #[wire_enum(graphql)]
/// #[derive(EnumIter, DeriveActiveEnum)]
/// #[sea_orm(rs_type = "String", db_type = "String(StringLen::None)")]
/// #[serde(rename_all = "lowercase")]
/// pub enum PostStatus {
///     #[sea_orm(string_value = "draft")]
///     Draft,
///     #[sea_orm(string_value = "published")]
///     Published,
/// }
/// # fn main() -> Result<(), serde_json::Error> {
/// # use async_graphql::{InputType, OutputType};
///
/// fn implements<T: Copy + Eq + std::fmt::Debug + schemars::JsonSchema + InputType + OutputType>() {}
/// implements::<PostStatus>();
/// assert_eq!(serde_json::to_value(PostStatus::Published)?, "published");
/// assert_eq!(serde_json::from_value::<PostStatus>("draft".into())?, PostStatus::Draft);
/// # Ok(())
/// # }
/// ```
pub use nest_rs_seaorm_macros::wire_enum;

/// Re-exported so an app resolves the framework's exact `sea_orm` pin, part of
/// this crate's API.
pub use sea_orm;
