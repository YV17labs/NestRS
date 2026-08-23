//! The `nest-rs-database` binding: [`SeaOrmDatabaseModule`], the bare import
//! that installs SeaORM's ambient executor around every unit of work.
//!
//! A folder rather than the crate root, so the module's own path names what it
//! is a module *of* — `seaorm/database/module.rs` reads `SeaOrmDatabaseModule`
//! before the file is opened. The pool it reads is the crate's, opened by
//! `SeaOrmModule` at the root; the rest of the crate is the ORM integration
//! this binding makes usable (`Repo`, the executor, the edge adapters).

mod module;

pub use module::SeaOrmDatabaseModule;
