//! SeaORM database integration.
//!
//! [`SeaOrmDatabaseModule`] is a [`DynamicModule`](nest_rs_core::DynamicModule) that
//! builds the pool in the collect phase and registers it as a
//! `sea_orm::DatabaseConnection`. Importing it also installs the `DbContext`
//! request interceptor, which binds each request to an ambient [`Executor`] —
//! the pool for a safe method, a transaction for a mutating one. Services then
//! query through [`Repo`] instead of holding a connection: every call runs on
//! the ambient executor (transactions need no hand-threading) and every read
//! is filtered by the caller's [`Ability`](nest_rs_authz::Ability) (row-level
//! security cannot be forgotten).
//!
//! ```ignore
//! #[module(imports = [SeaOrmDatabaseModule, UsersModule])]
//! pub struct AppModule;
//! ```
//!
//! Pin explicit values with [`SeaOrmDatabaseModule::for_root`]`(SeaOrmDatabaseConfig { .. })`.

#![warn(missing_docs)]

/// This crate's span target — Repository access — every query, and every row-level filter applied.
///
/// Declared by the crate that **owns** the concern, which is not always the only
/// crate emitting on it: a sibling and a `*-macros` expansion read this constant
/// rather than spelling a second one, because a target's one job is to say
/// **where** an event came from. A central table in the kernel would have meant
/// `nest-rs-core` holding a name for a concern it does not know exists.
pub const TARGET: &str = "nest_rs::orm";

mod database;
#[cfg(any(feature = "ws", feature = "mcp"))]
mod dispatch;
mod error;
mod executor;
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

pub use database::{
    SeaOrmDatabaseConfig, SeaOrmDatabaseModule, SeaOrmDatabaseSetup, connect_from_env,
};
pub use error::ServiceError;
#[cfg(feature = "http")]
pub use error::crud_error;
pub use executor::{
    CommitError, Executor, ExecutorScope, FinalizeOutcome, LazyTransaction, current_executor,
    current_executor_scope, with_executor, with_job_executor, with_request_executor,
};
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
pub use health::{DbHealthIndicator, SeaOrmHealthModule};
#[cfg(feature = "http")]
pub use http::{Bind, DbContext};

/// Re-exported so a consumer names one `sea_orm` — the framework's — instead of
/// carrying its own dependency and hand-mirroring the exact pin. SeaORM types
/// saturate the ORM public surface (`Repo` bounds, `Executor`, `DbErr`, the
/// entity / `ActiveModel` derives), so its version is part of this crate's API
/// contract: the workspace exact-pins it (`=2.0`) and apps should resolve it
/// through this re-export to stay in lockstep (the same rationale
/// `nest-rs-http` re-exports `poem` and `nest-rs-graphql` re-exports
/// `async_graphql`).
pub use sea_orm;

/// Re-exported so the `inventory::submit!` `#[expose(..., soft_delete)]` emits
/// resolves through this crate — the entity crate declares neither `inventory`
/// nor its version.
pub use inventory;
