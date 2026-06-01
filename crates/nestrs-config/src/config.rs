//! Namespaced, injectable configuration — the `registerAs` / `ConfigType` /
//! `ConfigModule.forFeature` trio, collapsed to the leverage Rust's type system
//! gives us: **the type is the token**.
//!
//! A `#[config(namespace = "database")]` struct (see the [`config`] macro)
//! implements [`Config`], so it loads itself from `NESTRS_DATABASE__*` and
//! validates on load. Importing [`ConfigModule::for_feature::<DatabaseConfig>()`]
//! in a module loads it once at boot and registers `Arc<DatabaseConfig>`, which
//! any provider in the app then injects directly:
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

use std::marker::PhantomData;

use nestrs_core::{ContainerBuilder, DynamicModule};
use serde::de::DeserializeOwned;
use validator::Validate;

use crate::environment::Environment;
use crate::loader::load_namespaced;
use crate::Result;

/// A namespaced configuration type. Implemented by the [`config`](crate::config)
/// macro, which supplies [`NAMESPACE`](Self::NAMESPACE); the default
/// [`load`](Self::load) reads `NESTRS_<NAMESPACE>__*` and validates. A framework
/// crate that needs a non-standard source (e.g. honouring the well-known
/// `DATABASE_URL` alongside the namespace) overrides `load`.
pub trait Config: DeserializeOwned + Validate + Send + Sync + Sized + 'static {
    /// The env-domain segment for this config: the `<DOMAIN>` in
    /// `NESTRS_<DOMAIN>__<KEY>`. Set by `#[config(namespace = "…")]`.
    const NAMESPACE: &'static str;

    /// Load and validate this config from the environment. The default reads the
    /// namespace prefix via [`load_namespaced`](crate::load_namespaced); a bad
    /// value or a violated `#[validate(...)]` rule returns an error that aborts
    /// the boot.
    fn load() -> Result<Self> {
        load_namespaced(Self::NAMESPACE)
    }
}

/// The configuration module — the `ConfigModule` analog and the **sole owner of
/// config loading**. List [`ConfigModule::for_root()`](Self::for_root) **first**
/// in the root module's imports: it reads the active [`Environment`] from
/// `NESTRS_ENV`, layers the `.env` cascade into the process environment (real env
/// vars always win), and registers `Arc<Environment>` as global infrastructure,
/// so every later [`Config`] load sees the merged environment:
///
/// ```ignore
/// #[module(imports = [ConfigModule::for_root(), DatabaseModule::for_root(), ...])]
/// pub struct AppModule;
/// ```
///
/// Its other entry point, [`for_feature`](Self::for_feature), is the **generic**
/// loader a configurable module routes through: `for_feature::<C>()` reads `C`'s
/// namespace, validates, and registers `Arc<C>` for injection. `ConfigModule`
/// stays agnostic of concrete config types — the module supplies `C`.
pub struct ConfigModule;

impl ConfigModule {
    /// Establish the config system: ensure the `.env` cascade is loaded, resolve
    /// the [`Environment`], and register `Arc<Environment>`. List it **first** in
    /// the root module's imports.
    pub fn for_root() -> ConfigRoot {
        ConfigRoot
    }

    /// Register a namespaced [`Config`] so it is injectable as `Arc<C>` anywhere
    /// in the app. List the returned value in `#[module(imports = [...])]`:
    ///
    /// ```ignore
    /// #[module(imports = [ConfigModule::for_feature::<DatabaseConfig>()])]
    /// pub struct UsersModule;
    /// ```
    ///
    /// The config loads in the **factory phase** (so a malformed environment
    /// fails the boot with a clear message), and — like every factory output —
    /// becomes global infrastructure, injectable from any module without a
    /// further import. A test that seeds `C` directly (`provide`/`override_value`)
    /// wins over this factory, so it never reads the real environment.
    pub fn for_feature<C: Config>() -> ConfigFeature<C> {
        ConfigFeature(PhantomData)
    }
}

/// The configured form of [`ConfigModule`] for one [`Config`] type, produced by
/// [`ConfigModule::for_feature`]. A [`DynamicModule`] whose only job is to queue
/// the config-loading factory in the collect phase.
pub struct ConfigFeature<C>(PhantomData<fn() -> C>);

impl<C: Config> DynamicModule for ConfigFeature<C> {
    // Loading is synchronous, but it is fallible, and `register` cannot return an
    // error — so the load is wrapped in a factory queued here and awaited by the
    // build, where a returned `Err` aborts the boot. (The same path the database
    // pool takes; config is one more piece of shared infrastructure.)
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide_factory::<C, _, _>(|_| async move {
            C::load().map_err(|e| {
                anyhow::anyhow!(
                    "invalid NESTRS_{}__* configuration: {e}",
                    C::NAMESPACE.to_ascii_uppercase()
                )
            })
        })
    }
}

/// Resolve the [`Environment`] and load the `.env` cascade **immediately**, at
/// the top of `main` — before anything that reads the environment outside the
/// DI graph (notably `Telemetry::init`, which runs before the app is built).
/// Returns the active environment. Idempotent with
/// [`ConfigModule::for_root`]: both fill only unset variables, so calling both is
/// harmless. An app with no pre-build env readers can rely on `for_root` alone.
///
/// ```ignore
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     let _env = nestrs_config::bootstrap_env();      // load .env first
///     let _t = nestrs_telemetry::Telemetry::init("api")?;  // now sees it
///     App::builder().module::<AppModule>()/* ... */.build().await?.run().await
/// }
/// ```
pub fn bootstrap_env() -> Environment {
    crate::dotenv::ensure_env_loaded();
    Environment::from_env()
}

/// The configured form of [`ConfigModule::for_root`]. A [`DynamicModule`] that
/// ensures the `.env` cascade is loaded and registers `Arc<Environment>`. The work
/// runs in the **collect phase** (a sync side effect + a direct `provide`), so it
/// completes before any [`Config`] factory reads the environment.
pub struct ConfigRoot;

impl DynamicModule for ConfigRoot {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        crate::dotenv::ensure_env_loaded();
        builder.provide(Environment::from_env())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConfigError;
    use serde::Deserialize;

    // A hand-written `impl Config` rather than the `#[config]` macro: the macro
    // emits `::nestrs_config::Config`, which a crate cannot resolve against
    // itself. The end-to-end macro + DI wiring is covered in `nestrs-testing`.
    #[derive(Debug, Deserialize, Validate, PartialEq)]
    struct DbCfg {
        url: String,
        #[validate(range(min = 1))]
        max_connections: u32,
    }
    impl Config for DbCfg {
        const NAMESPACE: &'static str = "testdb";
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn load_reads_the_namespace_prefix() {
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
    #[allow(clippy::result_large_err)]
    fn load_validates_on_the_way_in() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_TESTDB__URL", "postgres://localhost/app");
            jail.set_env("NESTRS_TESTDB__MAX_CONNECTIONS", "0");
            let err = DbCfg::load().expect_err("max_connections = 0 violates min = 1");
            assert!(matches!(err, ConfigError::Validation(_)));
            Ok(())
        });
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn load_does_not_read_a_sibling_namespace() {
        // A key under a different domain must not leak into this config.
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_OTHER__URL", "postgres://wrong");
            jail.set_env("NESTRS_TESTDB__MAX_CONNECTIONS", "3");
            let err = DbCfg::load().expect_err("url is unset under the testdb namespace");
            assert!(matches!(err, ConfigError::Source(_)));
            Ok(())
        });
    }
}
