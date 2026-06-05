//! SeaORM database integration.
//!
//! [`DatabaseModule`] is a [`DynamicModule`](nestrs_core::DynamicModule) that
//! builds the pool in the collect phase and registers it as a
//! `sea_orm::DatabaseConnection`. Importing it also installs the [`DbContext`]
//! request interceptor, which binds each request to an ambient [`Executor`] —
//! the pool for a safe method, a transaction for a mutating one. Services then
//! query through [`Repo`] instead of holding a connection: every call runs on
//! the ambient executor (transactions need no hand-threading) and every read
//! is filtered by the caller's [`Ability`](nestrs_authz::Ability) (row-level
//! security cannot be forgotten).
//!
//! ```ignore
//! #[module(imports = [DatabaseModule, UsersModule])]
//! pub struct AppModule;
//! ```
//!
//! Pin explicit values with [`DatabaseModule::for_root`]`(DatabaseConfig { .. })`.

mod config;
mod executor;
mod interceptor;
mod module;
mod page;
mod repo;
mod service;
mod worker;

#[cfg(feature = "http")]
mod bind;
#[cfg(feature = "graphql")]
pub mod graphql;
#[cfg(feature = "health")]
mod health;
#[cfg(feature = "ws")]
pub mod ws;

pub use config::DatabaseConfig;
pub use executor::{
    Executor, ExecutorScope, current_executor, current_executor_scope, with_executor,
    with_job_executor, with_request_executor,
};
pub use module::{DatabaseModule, DatabaseSetup};
pub use page::{Page, PageParams};
pub use repo::{Repo, scope_for};
pub use service::{Access, CreateModel, CrudService, UpdateModel};
pub use worker::WorkerDbContext;

#[cfg(feature = "http")]
pub use bind::Bind;
#[cfg(feature = "health")]
pub use health::{DatabaseHealthModule, DbHealthIndicator};

pub(crate) use interceptor::DbContext;
