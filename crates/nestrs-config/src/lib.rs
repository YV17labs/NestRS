//! Typed configuration loading for nestrs from the environment.
//!
//! A config is a namespaced [`Config`] struct that maps `NESTRS_<DOMAIN>__<KEY>`
//! variables to fields **explicitly** in its `from_env`, read through a
//! [`ConfigService`]; `ConfigModule` owns loading (the `.env` cascade + the
//! namespaced reader) and registers each config as `Arc<C>` for injection.

mod config;
mod dotenv;
mod environment;
mod error;
mod loader;
mod module;

pub use config::{Config, Namespaced};
pub use module::{ConfigFeature, ConfigModule, ConfigRoot};
pub use environment::Environment;
pub use error::{ConfigError, Result};
pub use loader::{env_var, ConfigService};

/// The `#[config(namespace = "…")]` decorator — marks a struct as a namespaced,
/// injectable [`Config`]. Re-exported from `nestrs-config-macros` so apps write
/// `nestrs_config::config`.
pub use nestrs_config_macros::config;
