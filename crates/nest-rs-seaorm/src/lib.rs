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

/// This crate's span target: every query, and every row-level filter applied.
pub const TARGET: &str = "nest_rs::orm";

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
#[cfg(feature = "http")]
pub use error::crud_error;
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

/// Re-exported so an app resolves the framework's exact `sea_orm` pin, part of
/// this crate's API.
pub use sea_orm;

/// Re-exported for the `inventory::submit!` that `#[expose(..., soft_delete)]`
/// emits: the entity crate does not declare `inventory`.
pub use inventory;
