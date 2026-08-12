//! `nestrs g <kind> <name>` generators — scaffold a feature port, a CRUD
//! resource, or a transport adapter, then auto-wire it into the current app.
//! Shared commit/wiring steps live in [`support`].

mod adapter;
mod auth;
mod cargo;
mod entity;
mod feature;
mod migration;
mod resource;
mod support;

/// `nestrs new` writes the same `migrations`/`seed` crates `g migration`
/// bootstraps — one implementation, reached without opening the module.
pub(crate) use migration::queue_db_crates;

pub use adapter::{AdapterOptions, run as run_adapter};
pub use auth::{AuthOptions, run as run_auth};
pub use entity::{EntityOptions, run as run_entity};
pub use feature::{FeatureOptions, run as run_feature};
pub use migration::{MigrationOptions, run as run_migration};
pub use resource::{ResourceOptions, run as run_resource};
