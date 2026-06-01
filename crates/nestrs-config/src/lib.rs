//! Configuration loading for nestrs — the `@nestjs/config` analog.
//!
//! Kept out of `nestrs-core` so the kernel (container, modules, lifecycle) does
//! not drag `figment` into every crate that depends on it; an app or framework
//! crate that needs configuration depends on `nestrs-config` directly.

mod config;
mod dotenv;
mod environment;
mod error;
mod loader;

pub use config::{bootstrap_env, Config, ConfigFeature, ConfigModule, ConfigRoot};
pub use environment::Environment;
pub use error::{ConfigError, Result};
pub use loader::{env_var, load, load_namespaced, load_validated};

/// The `#[config(namespace = "…")]` decorator — marks a struct as a namespaced,
/// injectable [`Config`]. Re-exported from `nestrs-config-macros` so apps write
/// `nestrs_config::config`.
pub use nestrs_config_macros::config;
