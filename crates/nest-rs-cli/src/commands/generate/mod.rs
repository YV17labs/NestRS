//! `nestrs g <kind> <name>` generators — scaffold a feature port, a CRUD
//! resource, or a transport adapter, then auto-wire it into the current app.
//! Shared commit/wiring steps live in [`support`].

mod adapter;
mod cargo;
mod feature;
mod migration;
mod resource;
mod support;

use support::{finish, resolve_start, wire_into_app};

pub use adapter::{AdapterOptions, run as run_adapter};
pub use feature::{FeatureOptions, run as run_feature};
pub use migration::{MigrationOptions, run as run_migration};
pub use resource::{ResourceOptions, run as run_resource};
