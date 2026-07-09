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
    const NAMESPACE: &'static str;
}

/// A namespaced configuration type.
///
/// [`from_env`](Self::from_env) is the **explicit** field-by-field mapping
/// from `NESTRS_<NAMESPACE>__<KEY>` variables, defaults included — the single
/// place to look for the env contract of a feature.
pub trait Config: Namespaced + Validate + Clone + Send + Sync + Sized + 'static {
    /// A set-but-unparseable variable returns `Err` (naming it) and aborts
    /// boot — never a silent fallback.
    fn from_env(env: &ConfigService) -> Result<Self>;

    fn load() -> Result<Self> {
        let env = ConfigService::for_namespace(Self::NAMESPACE);
        let config = Self::from_env(&env)?;
        config.validate()?;
        Ok(config)
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
