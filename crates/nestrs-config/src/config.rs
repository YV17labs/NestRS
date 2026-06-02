//! Namespaced, injectable configuration — the `registerAs` / `ConfigType` /
//! `ConfigModule.forFeature` trio, collapsed to the leverage Rust's type system
//! gives us: **the type is the token**.
//!
//! A `#[config(namespace = "database")]` struct supplies its namespace (via
//! [`Namespaced`]); the crate writes an `impl Config { fn from_env }` mapping each
//! `NESTRS_DATABASE__*` variable to a field. [`ConfigModule::for_feature::<DatabaseConfig>()`]
//! loads it once at boot and registers `Arc<DatabaseConfig>`, which any provider
//! then injects directly:
//!
//! ```ignore
//! #[module(imports = [ConfigModule::for_feature::<DatabaseConfig>()])]
//! pub struct UsersModule;
//!
//! #[injectable]
//! pub struct UsersService {
//!     #[inject] cfg: ::std::sync::Arc<DatabaseConfig>,   // ConfigType<…> + .KEY
//! }
//! ```

use validator::Validate;

use crate::loader::ConfigService;
use crate::Result;

/// The env-domain segment of a config — the `<DOMAIN>` in `NESTRS_<DOMAIN>__<KEY>`.
/// Supplied by the [`config`](crate::config) macro from `#[config(namespace = "…")]`,
/// so the namespace is declared once, on the struct. A supertrait of [`Config`].
pub trait Namespaced {
    /// The namespace, e.g. `"database"` → the `NESTRS_DATABASE__` prefix.
    const NAMESPACE: &'static str;
}

/// A namespaced configuration type — the typed source of truth for one concern.
///
/// The [`config`](crate::config) macro supplies the namespace (via
/// [`Namespaced`]); the crate writes [`from_env`](Self::from_env) — the
/// **explicit** mapping from `NESTRS_<NAMESPACE>__<KEY>` variables to fields,
/// field-by-field, defaults and all. That mapping is the single place to look:
/// opening a `config.rs` shows exactly which variable feeds which field and what
/// the value is when unset. `ConfigModule` owns the *resolution* (the `.env`
/// cascade, the namespaced reader); the module owns the *mapping* (`from_env`).
///
/// ```ignore
/// #[config(namespace = "database")]
/// #[derive(Clone, Debug, Default, Validate)]
/// pub struct DatabaseConfig { pub url: String, pub max_connections: Option<u32> }
///
/// impl Config for DatabaseConfig {
///     fn from_env(env: &ConfigService) -> Result<Self> {
///         Ok(Self {
///             url: env.get("URL").unwrap_or_default(),       // NESTRS_DATABASE__URL
///             max_connections: env.parse("MAX_CONNECTIONS")?, // … else None
///         })
///     }
/// }
/// ```
pub trait Config: Namespaced + Validate + Clone + Send + Sync + Sized + 'static {
    /// Map this config from the environment, explicitly. Read each field from its
    /// `NESTRS_<NAMESPACE>__<KEY>` variable via [`ConfigService`] (`env.get`,
    /// `env.parse`, `env.flag`, `env.list`), falling back to the field's default
    /// when unset. A variable that is set but unparseable returns `Err` (naming
    /// the variable) and aborts the boot — never a silent fallback.
    fn from_env(env: &ConfigService) -> Result<Self>;

    /// Resolve and validate from the environment: build the namespaced
    /// [`ConfigService`] (which ensures the `.env` cascade is loaded), run
    /// [`from_env`](Self::from_env), then the declarative `#[validate(...)]` rules.
    /// A bad value or a violated rule aborts the boot.
    fn load() -> Result<Self> {
        let env = ConfigService::for_namespace(Self::NAMESPACE);
        let config = Self::from_env(&env)?;
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
// `figment::Jail`'s closure returns figment's large `Result` — a fixed signature
// we cannot change, so the lint is unactionable on these tests.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;
    use crate::ConfigError;

    // A hand-written `impl Config` rather than the `#[config]` macro: the macro
    // emits `::nestrs_config::Config`, which a crate cannot resolve against
    // itself. The end-to-end macro + DI wiring is covered in `nestrs-testing`.
    #[derive(Clone, Validate, PartialEq, Debug)]
    struct DbCfg {
        url: String,
        #[validate(range(min = 1))]
        max_connections: u32,
    }
    impl Namespaced for DbCfg {
        const NAMESPACE: &'static str = "testdb";
    }
    impl Config for DbCfg {
        fn from_env(env: &ConfigService) -> Result<Self> {
            Ok(Self {
                url: env.get("URL").unwrap_or_default(),
                max_connections: env.parse("MAX_CONNECTIONS")?.unwrap_or(10),
            })
        }
    }

    #[test]
    fn load_maps_each_field_from_its_variable() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_TESTDB__URL", "postgres://localhost/app");
            jail.set_env("NESTRS_TESTDB__MAX_CONNECTIONS", "5");
            let cfg = DbCfg::load().expect("config loads from NESTRS_TESTDB__*");
            assert_eq!(
                cfg,
                DbCfg {
                    url: "postgres://localhost/app".into(),
                    max_connections: 5,
                }
            );
            Ok(())
        });
    }

    #[test]
    fn load_falls_back_to_defaults_when_unset() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_TESTDB__URL", "postgres://localhost/app");
            // MAX_CONNECTIONS unset → the in-mapping default (10).
            let cfg = DbCfg::load().expect("config loads with defaults");
            assert_eq!(cfg.max_connections, 10);
            Ok(())
        });
    }

    #[test]
    fn load_validates_on_the_way_in() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_TESTDB__MAX_CONNECTIONS", "0");
            let err = DbCfg::load().expect_err("max_connections = 0 violates min = 1");
            assert!(matches!(err, ConfigError::Validation(_)));
            Ok(())
        });
    }

    #[test]
    fn load_fails_loudly_on_an_unparseable_value() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_TESTDB__MAX_CONNECTIONS", "lots");
            let err = DbCfg::load().expect_err("non-numeric must abort the boot");
            assert!(
                matches!(err, ConfigError::Parse { ref var, .. } if var == "NESTRS_TESTDB__MAX_CONNECTIONS")
            );
            Ok(())
        });
    }
}
