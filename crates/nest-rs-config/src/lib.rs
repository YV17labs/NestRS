//! Typed configuration loading for nestrs from the environment.
//!
//! A config is a namespaced [`Config`] struct whose fields each read
//! `<PREFIX>_<DOMAIN>__<FIELD>`, the overlay `#[config]` writes from the struct
//! through a [`ConfigService`]; `ConfigModule` owns loading — the `.env`
//! cascade and the namespaced reader — and registers each config as `Arc<C>`
//! for injection.
//!
//! `<PREFIX>` is `NESTRS` out of the box. A deployment that wants its own brand
//! on its variables sets `NESTRS_ENV_PREFIX=ACME` on the process, and every name
//! here follows, `ACME_SEAORM__URL` through `ACME_ENV`.
//!
//! A resource an app opens more than once reads one namespace per instance:
//! `ConfigModule::provide_feature` takes a [`ForInstance`], and a named
//! instance's variables carry its name after the vendor
//! (`<PREFIX>_REDIS__CACHE__URL`, [`instance_namespace`]).
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
mod endpoint;
mod environment;
mod error;
mod field;
mod for_instance;
mod material;
mod module;
mod namespace;
mod service;
mod setting;
mod source;
#[cfg(test)]
mod testing;
#[cfg(feature = "tls")]
mod tls;
pub mod unclaimed;

pub use authorities::system_authorities;
pub use bounds::{Bound, BoundedDuration, DurationBounds, DurationUnit, Floor};
pub use config::{Config, Namespaced, read};
pub use dotenv::load_cascade;
pub use endpoint::endpoint_shown;
pub use environment::Environment;
pub use error::{ConfigError, ConfigNotResolvedError, Result};
pub use field::ConfigField;
pub use for_instance::ForInstance;
pub use material::{Material, read_material};
pub use module::{ConfigFeatureSetup, ConfigModule, ConfigRootSetup, ConfigSetup};
pub use namespace::instance_namespace;
pub use service::{ConfigService, spellings, spellings_or_field, var_name, var_name_or_field};
pub use setting::{FromSetting, Setting};
pub use source::{ConfigSource, EnvSource, MapSource, env_var};
#[cfg(feature = "tls")]
pub use tls::{ClientTls, TlsIdentity, TlsNames, TlsSetting};

/// The `#[config(namespace = "…")]` decorator — marks a struct as a namespaced,
/// injectable [`Config`] and writes its overlay: each field reads
/// `<PREFIX>_<NAMESPACE>__<FIELD>`, its name in SCREAMING_SNAKE, through the
/// reader its type chooses ([`ConfigField`]), over the base pinned in code or
/// the default. So the struct is the whole contract, and no field is
/// reachable from code alone.
///
/// The namespace is the stem of the file's path below `src/`, read and never
/// chosen (`redis/src/queue/config.rs` reads `"redis__queue"`). A field says
/// how it is read when its type cannot: `bounds = CONST` names the
/// [`DurationBounds`] a `Duration`, or an `Option<Duration>` whose `None` is
/// off, is held to, its key carrying the unit; `json` decodes a structured
/// value; `count` reads `0` as unlimited, `None`; `with = path` names a
/// `fn(&ConfigService, &str, T) -> Result<T>`. On the struct, `defaults =
/// path` names the profile-aware [`Config::defaults`], `validate = "manual"`
/// leaves the `Validate` derive to a hand-written impl, and `from_env =
/// "manual"` the whole `impl Config`, for an overlay whose fields depend on
/// each other.
///
/// ```
/// use std::time::Duration;
/// # use validator::Validate as _;
/// use nest_rs_config::{Bound, Config, ConfigService, DurationBounds, Floor, Namespaced, config};
///
/// const WINDOW: DurationBounds = DurationBounds::secs(
///     "WINDOW_SECS",
///     "ReportsConfig::window",
///     Floor::Units(Bound::new(1, "a shorter window counts nothing")),
///     Bound::new(3600, "past an hour the report is stale before it is read"),
/// );
///
/// #[config(namespace = "reports")]
/// #[derive(Clone, Debug)]
/// pub struct ReportsConfig {
///     #[validate(range(min = 1, max = 500))]
///     pub page_size: u32,
///     pub recipients: Vec<String>,
///     #[config(bounds = WINDOW)]
///     pub window: Duration,
/// }
///
/// impl Default for ReportsConfig {
///     fn default() -> Self {
///         Self { page_size: 50, recipients: Vec::new(), window: Duration::from_secs(60) }
///     }
/// }
///
/// assert_eq!(<ReportsConfig as Namespaced>::NAMESPACE, "reports");
/// let env = ConfigService::with_vars(
///     "reports",
///     [("PAGE_SIZE", "20"), ("RECIPIENTS", "ops@example.com"), ("WINDOW_SECS", "300")],
/// );
/// let read = ReportsConfig::from_env(&env, ReportsConfig::default())?;
/// assert_eq!((read.page_size, read.window), (20, Duration::from_secs(300)));
/// assert_eq!(read.recipients, ["ops@example.com"]);
/// assert!(ReportsConfig { page_size: 0, ..read }.validate().is_err());
/// # Ok::<(), nest_rs_config::ConfigError>(())
/// ```
pub use nest_rs_config_macros::config;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::bounds::{BoundedField, reads_field};
    pub use crate::dotenv::load_app_cascade;
    pub use crate::field::overlay_json;
    pub use crate::namespace::ConfigNamespace;
    #[cfg(feature = "tls")]
    pub use crate::tls::{PairRefusal, certified_key, crypto_provider};
    #[cfg(feature = "reqwest")]
    pub use crate::tls::{ReqwestTls, reqwest_tls};

    pub use nest_rs_core::inventory;
    pub use validator;
}
