//! The database binding: [`SeaOrmDatabaseModule`], the seam an app imports, and the
//! [`SeaOrmDatabaseConfig`] it resolves.
//!
//! A folder rather than the crate root, so the module's own path names what it
//! is a module *of* — `seaorm/database/module.rs` reads `SeaOrmDatabaseModule` before
//! the file is opened. The rest of the crate is the ORM integration this
//! binding makes usable (`Repo`, the executor, the edge adapters); this folder
//! is only the port it binds.

mod config;
mod module;

pub use config::SeaOrmDatabaseConfig;
pub use module::{SeaOrmDatabaseModule, SeaOrmDatabaseSetup, connect_from_env};
