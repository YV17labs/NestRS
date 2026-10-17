//! [`RedisModule`] — the substrate seam. `RedisModule::for_root(None)` opens
//! the [`RedisConnection`] this crate's bindings share; each binding
//! (`RedisQueueModule`, `RedisThrottlerModule`, `RedisScheduleModule`) is then a
//! bare import beside it, or its own `for_root` when it owns a config. An app
//! opening a second Redis names it with an instance marker:
//! `RedisModule::for_root((Cache, None))`, and a binding over a named instance
//! is a typed import, `RedisScheduleModule::<Locks>`.

use std::any::TypeId;
use std::borrow::Borrow;
use std::marker::PhantomData;

use nest_rs_config::{ConfigModule, ForInstance};
use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule, Instance, Unnamed};

use crate::{RedisConfig, RedisConnection};

/// The Redis substrate. Import [`RedisModule::for_root`] once per Redis the app
/// opens, then the bindings it needs beside it — they share the connection it
/// opens.
pub struct RedisModule<I = Unnamed>(PhantomData<fn() -> I>);

impl RedisModule {
    /// Open the connection of one instance. The default instance takes `x`, a
    /// named one `(Marker, x)` — `x` being `None`, which loads its
    /// [`RedisConfig`] from the environment, or a config, which pins the base
    /// those variables overlay, per field: the default reads
    /// `<PREFIX>_REDIS__*`, a `#[instance] struct Cache` reads
    /// `<PREFIX>_REDIS__CACHE__*` and is injected as
    /// `Arc<RedisConnection<Cache>>`.
    ///
    /// A binding over a named instance is a typed import: a value names the
    /// instance at `for_root`, a type argument at a binding.
    ///
    /// ```no_run
    /// use std::time::Duration;
    ///
    /// use nest_rs_core::{instance, module};
    /// use nest_rs_redis::{RedisConfig, RedisModule, RedisQueueModule, RedisThrottlerModule};
    ///
    /// #[instance]
    /// pub struct Cache;
    ///
    /// #[instance]
    /// pub struct Limits;
    ///
    /// #[module(imports = [
    ///     RedisModule::for_root(None),
    ///     RedisQueueModule,
    ///     RedisModule::for_root((Cache, None)),
    ///     RedisModule::for_root((Limits, RedisConfig {
    ///         connect_timeout: Duration::from_secs(2),
    ///         ..RedisConfig::defaults()
    ///     })),
    ///     RedisThrottlerModule::<Limits>,
    /// ])]
    /// pub struct AppModule;
    ///
    /// #[module(imports = [RedisModule::for_root(RedisConfig::defaults())])]
    /// pub struct PinnedModule;
    /// # use nest_rs_config::Config;
    /// ```
    pub fn for_root<I: Instance>(config: impl Into<ForInstance<I, RedisConfig>>) -> RedisSetup<I> {
        RedisSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`RedisModule::for_root`] for the instance
/// `I`. Resolves its [`RedisConfig`] and opens its [`RedisConnection`] in the
/// factory phase, before every binding's factory, wherever the bindings fall in
/// `imports = [..]`.
pub struct RedisSetup<I = Unnamed> {
    pinned: ForInstance<I, RedisConfig>,
}

impl<I: Instance> DynamicModule for RedisSetup<I> {
    fn module() -> TypeId {
        TypeId::of::<RedisModule<I>>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(
            self.pinned.clone(),
            builder.provide_meta(RedisConnection::<I>::declared_budget()),
        );
        builder.provide_factory_after::<RedisConnection<I>, (I::Of<RedisConfig>,), _, _>(
            |container| async move {
                let held = ConfigModule::resolved_for::<I, RedisConfig>(&container)?;
                let config: &RedisConfig = (*held).borrow();
                Ok(RedisConnection::<I>::open(config).await?)
            },
        )
    }
}
