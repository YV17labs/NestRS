//! Namespaced, injectable configuration: the **type is the token**.
//!
//! A `#[config(namespace = "…")]` struct supplies its namespace; the crate
//! writes `from_env` mapping each `<PREFIX>_<NAMESPACE>__*` variable to a field.
//! `ConfigModule::for_feature::<T>()` loads once at boot and registers
//! `Arc<T>`, injected directly by any provider.

use validator::Validate;

use crate::Result;
use crate::service::ConfigService;

/// The `<DOMAIN>` in `<PREFIX>_<DOMAIN>__<KEY>`. Supplied by the
/// [`config`](macro@crate::config) macro from `#[config(namespace = "…")]`.
pub trait Namespaced {
    /// The `<DOMAIN>` segment of every `<PREFIX>_<DOMAIN>__<KEY>` this type reads.
    const NAMESPACE: &'static str;

    /// The struct that declares the namespace — its module path and ident, as
    /// the decorator writes it. **Internal ABI**, compared against the link-time
    /// registry so a namespace two types declare is refused; empty on a
    /// hand-written impl, which then answers by its type name.
    #[doc(hidden)]
    const DECLARATION: &'static str = "";
}

/// Read `C`'s namespace over `base`, recording which variables it claimed.
///
/// The seam every path into `from_env` takes, a discovery registry's included;
/// a free function so a `Config` impl cannot override it and leave the claim
/// registry. It refuses first a namespace another type declares too
/// ([`ConfigError::SharedNamespace`](crate::ConfigError)), and runs the
/// unclaimed-variable report ([`crate::unclaimed`]) before a read error can end
/// the boot.
pub fn read<C: Config>(env: &ConfigService, base: C) -> Result<C> {
    crate::namespace::sole_declaration::<C>()?;
    let (value, claim) = crate::service::claiming::<C, _>(|| C::from_env(env, base));
    if env.reads_environment() {
        crate::unclaimed::after_read(env.namespace(), value.is_ok());
    }
    let value = value?;
    claim?;
    Ok(value)
}

/// A namespaced configuration type.
///
/// [`from_env`](Self::from_env) is the **explicit** field-by-field overlay of
/// `<PREFIX>_<NAMESPACE>__<KEY>` variables over a base value — the single place
/// to look for the env contract of a feature.
///
/// # The environment can always override, per field
///
/// A config pinned in code (`HttpModule::for_root(cfg)`) is the **base**, not
/// the answer: [`resolve`](Self::resolve) still runs `from_env` over it, per
/// **field**:
///
/// ```text
/// real env  >  pinned in code  >  .env cascade  >  Config::defaults()
/// ```
///
/// The one hard pin is seeding the value on the builder
/// (`App::builder().provide(cfg)`), for hermetic tests.
pub trait Config: Namespaced + Validate + Clone + Default + Send + Sync + Sized + 'static {
    /// Field-by-field overlay of this namespace's environment over `base`: every
    /// field takes its `<PREFIX>_<NAMESPACE>__<KEY>` value when the variable is
    /// set, and the matching field of `base` when it is not.
    ///
    /// A set-but-unparseable variable returns `Err` (naming it) and aborts
    /// boot — never a silent fallback.
    fn from_env(env: &ConfigService, base: Self) -> Result<Self>;

    /// The base the environment overlays when the call site pinned nothing.
    ///
    /// Defaults to [`Default::default`]. Override it when a field's safe
    /// baseline depends on the active profile: inside `from_env` it would also
    /// rewrite a pinned value.
    fn defaults() -> Self {
        Self::default()
    }

    /// Resolve this config for the boot: the environment overlaid on `pinned`
    /// when the call site supplied one, on [`defaults`](Self::defaults)
    /// otherwise, then validated. A pinned base is outranked by the deployment
    /// tier alone, an unpinned one by the `.env` cascade too.
    fn resolve(pinned: Option<Self>) -> Result<Self> {
        let env = ConfigService::for_namespace(Self::NAMESPACE);
        let (env, base) = match pinned {
            Some(pinned) => (env.over_pinned(), pinned),
            None => (env, Self::defaults()),
        };
        let config = read(&env, base)?;
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
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
mod tests {
    use super::*;
    use crate::ConfigError;
    use crate::service::var_name;

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
                url: env.get("URL")?.unwrap_or(base.url),
                max_connections: env
                    .parse("MAX_CONNECTIONS")?
                    .unwrap_or(base.max_connections),
            })
        }
    }

    #[test]
    fn load_maps_each_field_from_its_variable() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(var_name("testdb", "URL"), "postgres://localhost/app");
            jail.set_env(var_name("testdb", "MAX_CONNECTIONS"), "5");
            let cfg = DbCfg::load().expect("config loads from its namespace");
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
            jail.set_env(var_name("testdb", "URL"), "postgres://localhost/app");
            let cfg = DbCfg::load().expect("config loads with defaults");
            assert_eq!(cfg.max_connections, 10);
            Ok(())
        });
    }

    /// One namespace per test: `load_cascade`'s set-if-absent write and the
    /// `PUBLISHED` set are process-global.
    macro_rules! port_config {
        ($ty:ident, $ns:literal) => {
            #[derive(Clone, Validate)]
            struct $ty {
                port: u16,
            }
            impl Default for $ty {
                fn default() -> Self {
                    Self { port: 3000 }
                }
            }
            impl Namespaced for $ty {
                const NAMESPACE: &'static str = $ns;
            }
            impl Config for $ty {
                fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
                    Ok(Self {
                        port: env.parse("PORT")?.unwrap_or(base.port),
                    })
                }
            }
        };
    }

    port_config!(PinnedVsFileCfg, "pinfile");
    port_config!(PinnedVsDeployCfg, "pindeploy");

    /// A committed `.env` loses to a pin, asserted through the public `resolve`.
    #[test]
    fn a_committed_dotenv_file_loses_to_a_for_root_pin() {
        figment::Jail::expect_with(|jail| {
            // Built, not spelled: a literal name under a renamed prefix would
            // pass this test for the wrong reason.
            jail.create_file(".env", &format!("{}=3555", var_name("pinfile", "PORT")))?;
            crate::dotenv::load_cascade(std::path::Path::new("."), crate::Environment::Development);

            assert_eq!(
                PinnedVsFileCfg::load().expect("loads unpinned").port,
                3555,
                "with nothing pinned the cascade still outranks the in-code default",
            );
            assert_eq!(
                PinnedVsFileCfg::resolve(Some(PinnedVsFileCfg { port: 8080 }))
                    .expect("loads pinned")
                    .port,
                8080,
                "a `.env` committed beside the code must not silently undo a `for_root` pin",
            );
            Ok(())
        });
    }

    #[test]
    fn a_real_deployment_variable_outranks_a_for_root_pin() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(var_name("pindeploy", "PORT"), "3555");

            assert_eq!(
                PinnedVsDeployCfg::resolve(Some(PinnedVsDeployCfg { port: 8080 }))
                    .expect("loads pinned")
                    .port,
                3555,
                "the deployment is the last word — it is the tier the code cannot see",
            );
            Ok(())
        });
    }

    #[test]
    fn load_validates_on_the_way_in() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(var_name("testdb", "MAX_CONNECTIONS"), "0");
            let err = DbCfg::load().expect_err("max_connections = 0 violates min = 1");
            assert!(matches!(err, ConfigError::Validation { .. }));
            Ok(())
        });
    }

    #[test]
    fn load_fails_loudly_on_an_unparseable_value() {
        figment::Jail::expect_with(|jail| {
            jail.set_env(var_name("testdb", "MAX_CONNECTIONS"), "lots");
            let err = DbCfg::load().expect_err("non-numeric must abort the boot");
            assert!(
                matches!(err, ConfigError::Parse { ref var, .. } if *var == var_name("testdb", "MAX_CONNECTIONS"))
            );
            Ok(())
        });
    }
}
