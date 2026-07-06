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
mod module;
mod service;
mod source;

pub use config::{Config, Namespaced};
pub use dotenv::load_cascade;
pub use environment::Environment;
pub use error::{ConfigError, Result};
pub use module::{ConfigFeatureSetup, ConfigModule, ConfigRootSetup};
pub use service::ConfigService;
pub use source::{ConfigSource, EnvSource, env_var};

/// The `#[config(namespace = "…")]` decorator — marks a struct as a namespaced,
/// injectable [`Config`]. Re-exported from `nestrs-config-macros` so apps write
/// `nest_rs_config::config`.
pub use nest_rs_config_macros::config;
