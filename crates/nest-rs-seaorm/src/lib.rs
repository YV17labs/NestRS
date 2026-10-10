//! SeaORM for nestrs — the adapter that wraps `sea_orm`, whose URL scheme picks
//! the engine (postgres, mysql, sqlite). One substrate and its bindings:
//!
//! - [`SeaOrmModule::for_root`] resolves [`SeaOrmConfig`] (`<PREFIX>_SEAORM__*`)
//!   and opens the one `sea_orm::DatabaseConnection` every binding shares.
//! - [`SeaOrmDatabaseModule`] (bare) binds the `nest-rs-database` port: the
//!   `DbContext` request interceptor, which binds each request to an ambient
//!   [`Executor`] — the pool for a safe method, a transaction for a mutating
//!   one — and the `WorkerDbContext` bridge for jobs.
//! - [`SeaOrmHealthModule`] (bare, feature `health`) binds a health indicator.
//!
//! Services query through [`Repo`]: every call runs on the ambient executor and
//! every read is filtered by the caller's [`Ability`](nest_rs_authz::Ability).
//! An entity crosses the wire through [`macro@expose`], and a column's enum
//! through [`macro@wire_enum`].
//!
//! ```
//! # use nest_rs_core::module;
//! # use nest_rs_seaorm::sea_orm::{ConnectOptions, Database};
//! # use nest_rs_seaorm::{SeaOrmDatabaseModule, SeaOrmModule};
//! # use nest_rs_testing::TestApp;
//! # use nest_rs_worker::JobContext;
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
//! assert!(app.container().get_dyn::<dyn JobContext>().is_some());
//! # Ok(())
//! # }
//! ```
//!
//! Pin explicit values with [`SeaOrmModule::for_root`]`(SeaOrmConfig { .. })`.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod config;
mod database;
#[cfg(any(feature = "ws", feature = "mcp"))]
mod dispatch;
mod error;
mod executor;
mod module;
mod page;
mod repo;
pub mod retry;
mod service;
mod slug;
mod soft_delete;
pub mod target;
mod time;
mod worker;

#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "health")]
mod health;
#[cfg(feature = "http")]
mod http;
#[cfg(feature = "mcp")]
pub mod mcp;
#[cfg(feature = "ws")]
pub mod ws;

pub use config::SeaOrmConfig;
pub use database::SeaOrmDatabaseModule;
pub use error::{CommitError, ServiceError};
pub use executor::{
    Executor, ExecutorScope, FinalizeOutcome, LazyTransaction, current_executor,
    current_executor_scope, with_executor, with_job_executor, with_request_executor,
};
pub use module::{SeaOrmModule, SeaOrmSetup, connect_from_env};
pub use page::{DEFAULT_PAGE_SIZE, LIST_CAP, Page, PageParams, clamp_page_size};
pub use repo::{Repo, scope_for};
pub use service::{
    Access, Authorized, Creatable, CreateModel, CrudService, Deletable, Updatable, UpdateModel,
    model_uuid,
};
pub use slug::resolve_unique_slug;
pub use soft_delete::{
    SoftDeletable, SoftDeleteRegistration, audit_soft_delete_bindings, live_condition,
};
pub use time::now;
pub use worker::WorkerDbContext;

#[cfg(feature = "health")]
pub use health::{SeaOrmHealthIndicator, SeaOrmHealthModule};
#[cfg(feature = "http")]
pub use http::{Bind, DbContext};

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    #[cfg(feature = "http")]
    pub use crate::error::crud_error;

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

/// Re-exported for the `inventory::submit!` that `#[expose(..., soft_delete)]`
/// emits: the entity crate does not declare `inventory`.
pub use inventory;
