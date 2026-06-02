//! [`ConfigModule`] — the `ConfigModule.for_root` / `for_feature` DI wiring.

use std::marker::PhantomData;

use nestrs_core::{ContainerBuilder, DynamicModule};

use crate::config::Config;
use crate::environment::Environment;

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

    /// Wire a configurable module's `C` into `builder`, honouring an optional pin:
    /// `None` loads `C` from the environment (the [`for_feature`](Self::for_feature)
    /// path); `Some(cfg)` **provides `cfg` directly** so it wins over the
    /// environment (the value an app passes to `Module::for_root(config)`). The
    /// single helper every configurable module's `for_root` routes through.
    pub fn provide_feature<C: Config>(
        pinned: Option<C>,
        builder: ContainerBuilder,
    ) -> ContainerBuilder {
        match pinned {
            None => Self::for_feature::<C>().collect(builder),
            Some(config) => builder.provide(config),
        }
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
    // pool takes; config is one more piece of shared infrastructure.) The error
    // already names the offending variable (`ConfigError::Parse`) or the broken
    // rule, so it surfaces the misconfiguration directly.
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .provide_factory::<C, _, _>(|_| async move { C::load().map_err(anyhow::Error::from) })
    }
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
