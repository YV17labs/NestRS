//! SeaORM database integration for nestrs — the `@nestjs/typeorm` analog.
//!
//! [`DatabaseModule`] owns the connection. A database pool is built
//! asynchronously, which a synchronous [`Module`](nestrs_core::Module) cannot do,
//! so it is a [`DynamicModule`](nestrs_core::DynamicModule) that owns its
//! connection in the **collect phase**: declared in `#[module(imports = [...])]`
//! like any other module, it queues a factory that
//! [`AppBuilder::build`](nestrs_core::AppBuilder::build) `await`s before providers
//! are built. The pool is registered as a `sea_orm::DatabaseConnection`:
//!
//! ```ignore
//! #[module(imports = [
//!     DatabaseModule::for_root(DatabaseOptions {
//!         url: std::env::var("DATABASE_URL").unwrap_or_default(),
//!         ..Default::default()
//!     }),
//!     UsersModule,
//! ])]
//! pub struct AppModule;
//! ```
//!
//! Beyond the connection, the crate makes data access **transparent**. Importing
//! the module installs the [`DbContext`] request interceptor, which binds each
//! request to an ambient [`Executor`] — the pool for a safe method, a transaction
//! for a mutating one. A service then queries through [`Repo`] instead of holding
//! a connection: every call runs against that ambient executor (so transactions
//! need no hand-threading) and every read is filtered by the caller's
//! [`Ability`](nestrs_authz::Ability) (so row-level security cannot be forgotten).

mod executor;
mod interceptor;
mod module;
mod page;
mod repo;
mod service;

pub use executor::{current_executor, with_executor, Executor};
pub use module::{DatabaseModule, DatabaseOptions, DatabaseSetup};
pub use page::{Page, PageParams};
pub use repo::{scope_for, Repo};
pub use service::{Access, CreateModel, CrudService, UpdateModel};

pub(crate) use interceptor::DbContext;
