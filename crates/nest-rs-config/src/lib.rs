//! Typed configuration loading for nestrs from the environment.
//!
//! A config is a namespaced [`Config`] struct that maps
//! `<PREFIX>_<DOMAIN>__<KEY>` variables to fields **explicitly** in its
//! `from_env`, read through a [`ConfigService`]; `ConfigModule` owns loading
//! (the `.env` cascade + the namespaced reader) and registers each config as
//! `Arc<C>` for injection.
//!
//! `<PREFIX>` is `NESTRS` out of the box. A deployment that wants its own brand
//! on its variables sets `NESTRS_ENV_PREFIX=ACME` on the process, and every name
//! here follows, `ACME_DATABASE__URL` through `ACME_ENV`.

#![cfg_attr(not(test), deny(unsafe_code))]
#![warn(missing_docs)]

/// This crate's span target — The `.env` cascade, resolved namespaces, and refused values.
///
/// Declared by the crate that **owns** the concern, which is not always the only
/// crate emitting on it: a sibling and a `*-macros` expansion read this constant
/// rather than spelling a second one, because a target's one job is to say
/// **where** an event came from. A central table in the kernel would have meant
/// `nest-rs-core` holding a name for a concern it does not know exists.
pub const TARGET: &str = "nest_rs::config";

mod config;
mod dotenv;
mod environment;
mod error;
mod module;
mod service;
mod source;

pub use config::{Config, Namespaced, read};
pub use dotenv::load_cascade;
pub use environment::Environment;
pub use error::{ConfigError, Result};
pub use module::{ConfigFeatureSetup, ConfigModule, ConfigRootSetup, ConfigSetup};
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
