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
//! here follows, `ACME_SEAORM__URL` through `ACME_ENV`.
//!
//! A variable set under the prefix that no config reads is reported rather than
//! ignored — a misspelled key, or a near miss of a namespace the binary links:
//! other separators, another case, one misspelled segment ([`unclaimed`]).

#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

/// This crate's span target — The `.env` cascade, resolved namespaces, and refused values.
pub const TARGET: &str = "nest_rs::config";

mod authorities;
mod bounds;
mod config;
mod dotenv;
mod environment;
mod error;
mod material;
mod module;
mod namespace;
mod service;
mod setting;
mod source;
#[cfg(feature = "tls")]
mod tls;
pub mod unclaimed;

pub use authorities::system_authorities;
pub use bounds::{Bound, BoundedDuration, DurationBounds, DurationUnit, Floor};
pub use config::{Config, Namespaced, read};
pub use dotenv::load_cascade;
pub use environment::Environment;
pub use error::{ConfigError, Result};
pub use material::{Material, read_material};
pub use module::{ConfigFeatureSetup, ConfigModule, ConfigRootSetup, ConfigSetup};
pub use service::{ConfigService, spellings, var_name};
pub use setting::Setting;
pub use source::{ConfigSource, EnvSource, MapSource, env_var};
#[cfg(feature = "tls")]
pub use tls::{ClientTls, TlsIdentity, crypto_provider};

/// The `#[config(namespace = "…")]` decorator — marks a struct as a namespaced,
/// injectable [`Config`].
///
/// ```
/// # use validator::Validate as _;
/// use nest_rs_config::{Namespaced, config};
///
/// #[config(namespace = "seaorm")]
/// #[derive(Clone, Debug, serde::Deserialize)]
/// pub struct SeaOrmConfig {
///     pub url: String,
///     #[validate(range(min = 1))]
///     pub max_connections: u32,
/// }
///
/// assert_eq!(<SeaOrmConfig as Namespaced>::NAMESPACE, "seaorm");
/// let config = SeaOrmConfig { url: "postgres://localhost/app".into(), max_connections: 0 };
/// assert!(config.validate().is_err());
/// ```
pub use nest_rs_config_macros::config;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::namespace::ConfigNamespace;
    #[cfg(feature = "tls")]
    pub use crate::tls::{PairRefusal, certified_key};

    pub use nest_rs_core::inventory;
    pub use validator;
}
