//! [`ConfigModule`] — the `ConfigModule.for_root` / `for_feature` DI wiring.

use std::marker::PhantomData;

use nest_rs_core::{ContainerBuilder, DynamicModule};

use crate::config::Config;
use crate::environment::Environment;

/// Sole owner of config loading. List [`ConfigModule::for_root()`](Self::for_root)
/// **first** in the root module's imports — it merges the `.env` cascade (real
/// env vars always win) and registers `Arc<Environment>`, so every later
/// [`Config`] load sees the merged environment.
pub struct ConfigModule;

impl ConfigModule {
    pub fn for_root() -> ConfigRoot {
        ConfigRoot
    }

    /// Loads in the **factory phase**, becoming global infrastructure. A test
    /// that seeds `C` directly wins over this factory.
    pub fn for_feature<C: Config>() -> ConfigFeature<C> {
        ConfigFeature(PhantomData)
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

pub struct ConfigFeature<C>(PhantomData<fn() -> C>);

impl<C: Config> DynamicModule for ConfigFeature<C> {
    // Loading is sync-but-fallible and `register` cannot return an error, so
    // we queue a factory the build awaits — an Err there aborts boot with the
    // variable named.
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .provide_factory::<C, _, _>(|_| async move { C::load().map_err(anyhow::Error::from) })
    }
}

pub struct ConfigRoot;

impl DynamicModule for ConfigRoot {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        crate::dotenv::ensure_env_loaded();
        builder.provide(Environment::from_env())
    }
}
