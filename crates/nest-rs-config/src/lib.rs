//! Typed configuration loading for nestrs from the environment.
//!
//! A config is a namespaced [`Config`] struct that maps
//! `<PREFIX>_<DOMAIN>__<KEY>` variables to fields **explicitly** in its
//! `from_env`, read through a [`ConfigService`]; `ConfigModule` owns loading
//! (the `.env` cascade + the namespaced reader) and registers each config as
//! `Arc<C>` for injection.
//!
//! `<PREFIX>` is `NESTRS` out of the box. An app that wants its own brand on
//! its deployment variables declares it once — `nest_rs::env_prefix!("ACME")` —
//! and every name here follows, `ACME_DATABASE__URL` through `ACME_ENV`.
#![cfg_attr(not(test), deny(unsafe_code))]
#![warn(missing_docs)]

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
pub use service::{ConfigService, var_name};
pub use source::{ConfigSource, EnvSource, MapSource, env_var};

/// The `#[config(namespace = "…")]` decorator — marks a struct as a namespaced,
/// injectable [`Config`]. Re-exported from `nest-rs-config-macros` so apps write
/// `nest_rs_config::config`.
pub use nest_rs_config_macros::config;

// `#[config]` injects the `Validate` derive and its `crate = ` override, so a
// `#[config]` struct needs no `validator` line and no version to align.
#[doc(hidden)]
pub use validator;
