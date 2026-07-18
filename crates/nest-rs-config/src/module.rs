//! [`ConfigModule`] — the `ConfigModule.for_root` / `for_feature` DI wiring.

use std::marker::PhantomData;

use nest_rs_core::{ContainerBuilder, DynamicModule};

use crate::config::Config;
use crate::environment::Environment;

/// Sole owner of config loading. List [`ConfigModule::for_root()`](Self::for_root)
/// **first** in the root module's imports — it makes the `.env` cascade visible
/// to config reads (real env vars always win; resolution goes through an
/// in-crate map, so the process env is **never** mutated) and registers
/// `Arc<Environment>`, so every later [`Config`] load sees dotenv values.
pub struct ConfigModule;

impl ConfigModule {
    /// Import first in the root module — makes the `.env` cascade visible to
    /// config reads and registers `Arc<Environment>`.
    pub fn for_root() -> ConfigRootSetup {
        ConfigRootSetup
    }

    /// Loads in the **factory phase**, becoming global infrastructure. A test
    /// that seeds `C` directly wins over this factory.
    pub fn for_feature<C: Config>() -> ConfigFeatureSetup<C> {
        ConfigFeatureSetup(PhantomData)
    }

    /// `None` loads from the environment; `Some(cfg)` pins the value (what an
    /// app passes to `Module::for_root(config)`). Every configurable module's
    /// `for_root` routes through this.
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

/// The import produced by [`ConfigModule::for_feature`]. Queues a factory that
/// loads and validates `C` in the factory phase, as global infrastructure.
pub struct ConfigFeatureSetup<C>(PhantomData<fn() -> C>);

impl<C: Config> DynamicModule for ConfigFeatureSetup<C> {
    // Loading is sync-but-fallible and `register` cannot return an error, so
    // we queue a factory the build awaits — an Err there aborts boot with the
    // variable named.
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .provide_factory::<C, _, _>(|_| async move { C::load().map_err(anyhow::Error::from) })
    }
}

/// The import produced by [`ConfigModule::for_root`]. Registers the active
/// [`Environment`] so later config loads see the resolved `.env` cascade.
pub struct ConfigRootSetup;

impl DynamicModule for ConfigRootSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        // `Environment::from_env` reads `NESTRS_ENV` from the real process env;
        // dotenv values reach config reads lazily via `env_var` (the in-crate
        // map), so collect mutates no process state — no `set_var` on the boot
        // path that a spawned worker's `getenv` could race.
        builder.provide(Environment::from_env())
    }
}
