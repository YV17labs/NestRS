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
//! `ConfigModule::provide_root` takes a [`ForInstance`], and a named
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
mod surface;
#[cfg(test)]
mod testing;
#[cfg(feature = "tls")]
mod tls;
pub mod unclaimed;

pub use authorities::system_authorities;
pub use bounds::{
    Bound, BoundedField, BoundedSetting, CountBounds, DurationBounds, DurationUnit, Floor,
};
pub use config::{Config, Namespaced, read};
pub use dotenv::load_cascade;
pub use endpoint::endpoint_shown;
pub use environment::Environment;
pub use error::{ConfigError, ConfigNotResolvedError, Result};
pub use field::{ConfigField, Field};
pub use for_instance::ForInstance;
pub use material::{Material, read_material};
pub use module::{ConfigModule, ConfigSetup};
pub use namespace::instance_namespace;
pub use service::{ConfigService, spellings, spellings_or_field, var_name, var_name_or_field};
pub use setting::{FromSetting, Setting};
pub use source::{ConfigSource, EnvSource, MapSource, env_var};
pub use surface::Surface;
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
/// how it is read when its type cannot: `bounds = RANGE` names the range — a
/// [`DurationBounds`] for a `Duration` or an `Option<Duration>`, a
/// [`CountBounds`] for a `usize`, a `u32` or an `Option<usize>`, an `Option`'s
/// `None` off — and binds it to the field as the associated [`BoundedSetting`]
/// named for it (`ReportsConfig::WINDOW`), its key the field's name with a
/// duration's unit after it; `json` decodes a structured value; `with = path`
/// names a `fn(&ConfigService, Field, T) -> Result<T>`, handed the field's
/// binding. Every other field is bound too, as the associated [`Field`] named for
/// it (`ReportsConfig::RECIPIENTS`), so a sentence naming a field or a check of a
/// value set in code reads its key and path, never types them. On the struct, `validate = "manual"` leaves the
/// `Validate` derive to a hand-written impl, and `from_env = "manual"` the
/// whole `impl Config`, for an overlay whose fields depend on each other.
///
/// ```
/// use std::time::Duration;
/// # use validator::Validate as _;
/// use nest_rs_config::{
///     Bound, Config, ConfigService, CountBounds, DurationBounds, Floor, Namespaced, config,
/// };
///
/// const WINDOW: DurationBounds = DurationBounds::secs(
///     Floor::Units(Bound::new(1, "a shorter window counts nothing")),
///     Bound::new(3600, "past an hour the report is stale before it is read"),
/// );
///
/// const PAGE_SIZE: CountBounds = CountBounds::new(
///     Bound::new(1, "an empty page reports nothing"),
///     Bound::new(500, "past it a page outgrows the mail that carries it"),
/// );
///
/// #[config(namespace = "reports")]
/// #[derive(Clone, Debug)]
/// pub struct ReportsConfig {
///     #[config(bounds = PAGE_SIZE)]
///     pub page_size: u32,
///     #[validate(length(min = 1))]
///     pub recipients: Vec<String>,
///     #[config(bounds = WINDOW)]
///     pub window: Duration,
/// }
///
/// impl Default for ReportsConfig {
///     fn default() -> Self {
///         Self { page_size: 50, recipients: vec!["ops@example.com".into()], window: Duration::from_secs(60) }
///     }
/// }
///
/// assert_eq!(<ReportsConfig as Namespaced>::NAMESPACE, "reports");
/// assert_eq!(ReportsConfig::WINDOW.key(), "WINDOW_SECS");
/// assert_eq!(ReportsConfig::RECIPIENTS.key(), "RECIPIENTS");
/// assert_eq!(ReportsConfig::RECIPIENTS.path(), "ReportsConfig::recipients");
/// let env = ConfigService::with_vars(
///     "reports",
///     [("PAGE_SIZE", "20"), ("RECIPIENTS", "ops@example.com"), ("WINDOW_SECS", "300")],
/// );
/// let read = ReportsConfig::from_env(&env, ReportsConfig::default())?;
/// assert_eq!((read.page_size, read.window), (20, Duration::from_secs(300)));
/// assert_eq!(read.recipients, ["ops@example.com"]);
/// assert!(ReportsConfig { recipients: Vec::new(), ..read }.validate().is_err());
/// # Ok::<(), nest_rs_config::ConfigError>(())
/// ```
pub use nest_rs_config_macros::config;

#[doc(hidden)]
pub mod __private {
    //! Called by this framework's macro expansions and sibling crates. Not API:
    //! may change in any release.

    pub use crate::dotenv::load_app_cascade;
    pub use crate::endpoint::is_loopback;
    pub use crate::field::overlay_json;
    pub use crate::module::{ConfigFeatureSetup, check_feature, feature, queue_feature};
    pub use crate::namespace::ConfigNamespace;
    #[cfg(feature = "tls")]
    pub use crate::tls::{PairRefusal, certified_key, crypto_provider};
    #[cfg(feature = "reqwest")]
    pub use crate::tls::{ReqwestTls, reqwest_tls};
    pub use crate::unclaimed::__private::{RetiredKey, RetiredKeys};

    pub use nest_rs_core::__private::InjectedFactory;
    pub use nest_rs_core::inventory;
    pub use validator;
}
