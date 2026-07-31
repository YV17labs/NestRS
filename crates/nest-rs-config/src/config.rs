//! Namespaced, injectable configuration: the **type is the token**.
//!
//! A `#[config(namespace = "…")]` struct supplies its namespace; the crate
//! writes `from_env` mapping each `NESTRS_<NAMESPACE>__*` variable to a field.
//! `ConfigModule::for_feature::<T>()` loads once at boot and registers
//! `Arc<T>`, injected directly by any provider.

use validator::Validate;

use crate::Result;
use crate::service::ConfigService;

/// The `<DOMAIN>` in `NESTRS_<DOMAIN>__<KEY>`. Supplied by the [`config`](crate::config)
/// macro from `#[config(namespace = "…")]`.
pub trait Namespaced {
    /// The `<DOMAIN>` segment of every `NESTRS_<DOMAIN>__<KEY>` this type reads.
    const NAMESPACE: &'static str;
}

/// A namespaced configuration type.
///
/// [`from_env`](Self::from_env) is the **explicit** field-by-field overlay of
/// `NESTRS_<NAMESPACE>__<KEY>` variables over a base value — the single place to
/// look for the env contract of a feature.
///
/// # The environment can always override, per field
///
/// A module configured in code (`HttpModule::for_root(HttpConfig { port: 3000,
/// ..Default::default() })`) hands that struct in as the **base**, not as the
/// answer: [`resolve`](Self::resolve) still runs `from_env` over it, so
/// `NESTRS_HTTP__TLS_CERT` reaches an app whose author only meant to pin the
/// port. The whole precedence chain is
///
/// ```text
/// real env  >  pinned in code  >  .env cascade  >  Config::defaults()
/// ```
///
/// and it is per **field**, never per struct — which is what makes the
/// framework's dual-path rule true rather than aspirational. The one hard pin
/// left is seeding the value on the builder (`App::builder().provide(cfg)`): a
/// seed short-circuits the factory `resolve` runs in, which is the escape hatch
/// a test wanting hermetic values takes.
pub trait Config: Namespaced + Validate + Clone + Default + Send + Sync + Sized + 'static {
    /// Field-by-field overlay of this namespace's environment over `base`: every
    /// field takes its `NESTRS_<NAMESPACE>__<KEY>` value when the variable is
    /// set, and the matching field of `base` when it is not. One body serves
    /// both the pinned and the unpinned path, so no field can be reachable one
    /// way only.
    ///
    /// A set-but-unparseable variable returns `Err` (naming it) and aborts
    /// boot — never a silent fallback.
    fn from_env(env: &ConfigService, base: Self) -> Result<Self>;

    /// The base the environment overlays when the call site pinned nothing.
    ///
    /// Defaults to [`Default::default`]. Override it when a field's *safe*
    /// baseline depends on the active profile rather than being a constant —
    /// `StorageConfig::allow_http`, `OpenApiConfig::enabled` and
    /// `QueueConfig::url` all default one way in dev and another in
    /// staging/production, and that belongs here rather than inside `from_env`
    /// where it would also silently rewrite a pinned value.
    fn defaults() -> Self {
        Self::default()
    }

    /// Resolve this config for the boot: the environment overlaid on `pinned`
    /// when the call site supplied one, on [`defaults`](Self::defaults)
    /// otherwise, then validated. The single entry point
    /// `ConfigModule::provide_feature` calls.
    fn resolve(pinned: Option<Self>) -> Result<Self> {
        let env = ConfigService::for_namespace(Self::NAMESPACE);
        // A pinned base is deliberate, so only the deployment tier outranks it;
        // an unpinned base is a default, so the `.env` cascade outranks it too.
        let (env, base) = match pinned {
            Some(pinned) => (env.over_pinned(), pinned),
            None => (env, Self::defaults()),
        };
        let config = Self::from_env(&env, base)?;
        // Tagged with the namespace here rather than through a blanket `From`:
        // the namespace is what tells an operator *which* config failed when
        // several are loaded, and only this call site knows it.
        config
            .validate()
            .map_err(|errors| crate::ConfigError::validation(Self::NAMESPACE, errors))?;
        Ok(config)
    }

    /// Read from the environment for this type's namespace with nothing pinned,
    /// and validate the result.
    fn load() -> Result<Self> {
        Self::resolve(None)
    }
}

#[cfg(test)]
// figment::Jail's fixed closure signature triggers this lint unactionably.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;
    use crate::ConfigError;

    // Hand-written impl: the macro emits ::nest_rs_config::Config which a crate
    // cannot resolve against itself. End-to-end wiring is covered in nest-rs-testing.
    #[derive(Clone, Validate, PartialEq, Debug)]
    struct DbCfg {
        url: String,
        #[validate(range(min = 1))]
        max_connections: u32,
    }
    impl Default for DbCfg {
        fn default() -> Self {
            Self {
                url: String::new(),
                max_connections: 10,
            }
        }
    }
    impl Namespaced for DbCfg {
        const NAMESPACE: &'static str = "testdb";
    }
    impl Config for DbCfg {
        fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
            Ok(Self {
                url: env.get("URL").unwrap_or(base.url),
                max_connections: env
                    .parse("MAX_CONNECTIONS")?
                    .unwrap_or(base.max_connections),
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
            assert!(matches!(err, ConfigError::Validation { .. }));
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
