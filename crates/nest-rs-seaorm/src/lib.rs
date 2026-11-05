//! SeaORM for nestrs — the adapter that wraps `sea_orm` over PostgreSQL, the
//! one engine it supports, as CI runs it. One substrate and its bindings:
//!
//! - [`SeaOrmModule`] resolves [`SeaOrmConfig`] (`<PREFIX>_SEAORM__*`)
//!   and opens the one `sea_orm::DatabaseConnection` every binding shares,
//!   closing it when the app ends, after every shutdown hook, without waiting
//!   past the settle for a connection no unit holds. Under the `health`
//!   feature it attaches the pool's indicator, `seaorm`, which an app
//!   importing `HealthModule` runs on the readiness and startup probes.
//! - [`SeaOrmDatabaseModule`] (bare), which imports the pool it runs on, binds
//!   the `nest-rs-database` port: [`SeaOrmUnitOfWork`], which every edge — an
//!   HTTP request, a WS message, an MCP operation, a queue attempt, a scheduled
//!   tick — and every lifecycle hook runs its unit of work through, on an ambient
//!   [`SeaOrmExecutor`]: the pool, or a transaction opened on the unit's first
//!   data-layer touch.
//! - Under the `queue` feature, [`SeaOrmDatabaseModule`] also keeps the queue
//!   port's outbox, `queue::SeaOrmOutbox`: a job pushed inside a unit's
//!   transaction is written there and filed after its commit;
//!   `queue::OutboxMigration` creates its table.
//!
//! Services query through [`Repo`]: every call runs on the ambient executor,
//! every read is filtered by the caller's [`Ability`](nest_rs_authz::Ability),
//! and an entity declared `#[expose(..., soft_delete)]` keeps its deleted rows
//! out of every read but [`Repo::with_tombstones`].
//! [`nest_rs_database::atomic`] runs a block inside a [`Savepoint`] of that
//! executor's transaction, or a transaction of its own on the pool.
//! An entity crosses the wire through [`macro@expose`], and a column's enum
//! through `nest_rs_core`'s `#[wire_enum]`.
//!
//! ```
//! # use nest_rs_core::module;
//! # use nest_rs_seaorm::sea_orm::{ConnectOptions, Database};
//! # use nest_rs_seaorm::SeaOrmDatabaseModule;
//! # use nest_rs_testing::TestApp;
//! # use nest_rs_database::UnitOfWork;
//! # #[module(providers = [])]
//! # pub struct UsersModule;
//! #[module(imports = [SeaOrmDatabaseModule, UsersModule])]
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
//! Pin a base those variables overlay with
//! [`SeaOrmModule::for_root`]`(SeaOrmConfig { .. })`, the one reason an app
//! running on the binding names the pool.

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

mod atomic;
mod bind;
mod column_default;
mod config;
mod database;
mod error;
mod executor;
mod exposed;
mod insertion;
mod module;
mod page;
mod repo;
pub mod retry;
mod savepoint;
mod select;
mod service;
mod slug;
mod soft_delete;
pub mod target;
mod time;
mod turn;
mod unique_key;
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
pub use executor::{FinalizeOutcome, LazyTransaction, SeaOrmExecutor};
pub use exposed::Exposed;
pub use insertion::Insertion;
pub use module::{SeaOrmModule, SeaOrmSetup, connect_from_env};
pub use page::{DEFAULT_PAGE_SIZE, LIST_CAP, Page, PageCursor, PageParams, clamp_page_size};
pub use repo::{Repo, scope_for};
pub use savepoint::Savepoint;
pub use select::RepoSelect;
pub use service::{
    Access, Authorized, Creatable, CreateModel, CrudService, Deletable, Updatable, UpdateModel,
    model_uuid,
};
pub use slug::resolve_unique_slug;
pub use soft_delete::{SoftDeletable, live_condition};
pub use time::now;
pub use unit::SeaOrmUnitOfWork;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::bind::resource_bind;
    pub use crate::config::DEFAULT_STATEMENT_TIMEOUT;
    pub use crate::error::masking;
    #[cfg(feature = "graphql")]
    pub use crate::error::{graphql_crud_error, graphql_crud_write_error};
    #[cfg(feature = "http")]
    pub use crate::error::{http_crud_error, http_crud_write_error};
    pub use crate::executor::current_executor;
    #[cfg(feature = "graphql")]
    pub use crate::graphql::bind::graphql_bind;
    #[cfg(feature = "graphql")]
    pub use crate::graphql::relations::__private::{relation_key, relation_page};
    #[cfg(feature = "http")]
    pub use crate::http::http_bind;
    #[cfg(feature = "graphql")]
    pub use crate::page::graphql_page_cursor;
    pub use crate::repo::unit_of_work_required;
    pub use crate::service::{SameEntity, bound_entity, created_key};

    pub use crate::column_default::ColumnDefaultRegistration;
    pub use crate::exposed::ExposeDeclared;
    pub use crate::soft_delete::{SoftDeleteDeclared, SoftDeleteRegistration};
    pub use crate::unique_key::UniqueKeyRegistration;
    pub use inventory;

    // What `#[expose]` emits, so an entity crate declares none of these; each
    // derive carries a `crate = ` override pointing here.
    pub use async_trait::async_trait;
    pub use chrono;
    #[cfg(feature = "graphql")]
    pub use nest_rs_graphql::__private::{
        FieldFailure, GraphqlRelationRegistration, LoaderOwner, seeded_loader,
    };
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
/// inputs, and the round trip response masking reads a row through, each
/// column by its own serde impls. The `Model` itself derives no serde — a
/// `Serialize` or `Deserialize` derived on it is refused — so it serializes
/// nowhere.
///
/// **Exposure is opt-in:** a column reaches the wire only when its field
/// carries `#[expose]` (or `#[expose(input(...))]`, which implies read). A field
/// with no `#[expose]` stays hidden on every transport, so a column added by a
/// later migration never leaks by omission.
///
/// ```
/// # use nest_rs_seaorm::expose;
/// # mod service {
/// #     pub struct UsersService;
/// # }
/// # mod user {
/// #     use super::*;
/// #     use nest_rs_core::Secret;
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
///     pub password_hash: Option<Secret<String>>,                 // no #[expose] ⇒ hidden
/// }
/// # impl ActiveModelBehavior for ActiveModel {}
/// # }
/// # fn main() -> Result<(), serde_json::Error> {
/// # use sea_orm::prelude::Uuid;
/// # use user::{CreateUser, Model, UpdateUser, User};
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
/// # Ok(())
/// # }
/// ```
///
/// The `Model` is no wire type: a handler answers its exposed type, so a
/// hidden column never crosses a route no mask shapes.
///
/// ```compile_fail,E0277
/// # use nest_rs_seaorm::expose;
/// # mod post {
/// #     use super::*;
/// #     use sea_orm::entity::prelude::*;
/// #[expose(name = "Post")]
/// # #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
/// # #[sea_orm(table_name = "posts")]
/// pub struct Model {
///     #[sea_orm(primary_key)]
///     #[expose]
///     pub id: i32,
///     pub deleted_at: Option<DateTimeWithTimeZone>,
/// }
/// #     #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
/// #     pub enum Relation {}
/// #     impl ActiveModelBehavior for ActiveModel {}
/// # }
/// # let row = post::Model { id: 1, deleted_at: None };
/// serde_json::to_value(&row)?;
/// # Ok::<(), serde_json::Error>(())
/// ```
///
/// A masked reply rebuilds every hidden column, so each takes a placeholder:
/// a `String`, `Option`, `bool` or number its `Default`, a `Secret` its
/// value's, and any other `#[wire_default]` (its type's `Default`) or
/// `#[wire_default(expr)]`. A hidden column with none, and a hidden primary
/// key, are refused at the field.
///
/// Add the `graphql` flag, with this crate's `graphql` feature, and the DTO is
/// also a GraphQL object, its `#[expose]`d relations resolved through
/// dataloaders (`graphql::RelatedTo`); without the flag the entity crate
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
/// # fn main() {
/// # use async_graphql::OutputType;
///
/// fn wire_only<T: serde::Serialize + schemars::JsonSchema>() {}
/// fn with_graphql<T: serde::Serialize + schemars::JsonSchema + OutputType>() {}
/// wire_only::<rest::Item>();
/// with_graphql::<graph::Item>();
/// # }
/// ```
///
/// **An insert through [`Repo`] fills the entity's column defaults** the row
/// left unset, [`Repo::insert_unscoped`] and [`Creatable::create`] alike: a
/// `Uuid` key with a v7 UUID, and a column's `#[expose(default = expr)]` — a
/// server-side constant on a column the create input does not carry. What the
/// caller's grants pin (the tenant, the author) [`Creatable::create`] fills
/// first, so a pin wins over a default. The entity's `ActiveModelBehavior` is
/// its own, but under `timestamps`, which writes it to stamp `created_at` on
/// insert and `updated_at`, when the entity has one, on every save.
pub use nest_rs_seaorm_macros::expose;

/// Re-exported so an app resolves the framework's exact `sea_orm` pin, part of
/// this crate's API.
pub use sea_orm;
