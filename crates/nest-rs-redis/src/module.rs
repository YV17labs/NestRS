//! [`RedisModule`] — the substrate seam. `RedisModule` opens the
//! [`RedisConnection`] this crate's bindings share, and under the `health`
//! feature attaches its indicator; each binding (`RedisQueueModule`,
//! `RedisThrottlerModule`, `RedisScheduleModule`) is then a bare import beside
//! it. An app opening a second Redis names it with an instance marker, as a
//! typed import, `RedisModule::<Cache>`, and a binding over a named instance
//! the same way, `RedisScheduleModule::<Locks>`.

use std::borrow::Borrow;
use std::marker::PhantomData;

use nest_rs_config::{ConfigModule, ConfigSetup, ForInstance};
use nest_rs_core::{Collecting, ContainerBuilder, Instance, Module, Registering, Unnamed};

use crate::{RedisConfig, RedisConnection};

/// The Redis substrate for the instance `I`: the connection, its budget and,
/// under the `health` feature, its indicator. Import it once per Redis the app
/// opens — bare to read its variables, through [`RedisModule::for_root`] to
/// pin — then the bindings it needs beside it, which share the connection it
/// opens. It is imported by type, so two imports of one instance open and
/// report one connection.
///
/// The default instance reads `<PREFIX>_REDIS__*`; a `#[instance] struct
/// Cache` reads `<PREFIX>_REDIS__CACHE__*` and is injected as
/// `Arc<RedisConnection<Cache>>`.
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
///     RedisModule,
///     RedisQueueModule,
///     RedisModule::<Cache>,
///     RedisModule::for_root((Limits, RedisConfig {
///         connect_timeout: Duration::from_secs(2),
///         ..RedisConfig::default()
///     })),
///     RedisThrottlerModule::<Limits>,
/// ])]
/// pub struct AppModule;
/// ```
pub struct RedisModule<I = Unnamed>(PhantomData<fn() -> I>);

impl RedisModule {
    /// Pin the config of one instance, the base its variables overlay, per
    /// field: the default instance takes a [`RedisConfig`], a named one
    /// `(Marker, config)`. The bare import, `RedisModule` or
    /// `RedisModule::<Marker>`, reads those variables over the defaults.
    pub fn for_root<I: Instance>(config: impl Into<ForInstance<I, RedisConfig>>) -> RedisSetup<I> {
        ConfigModule::setup(config)
    }
}

/// The configured import [`RedisModule::for_root`] returns for the instance
/// `I`: its connection opens in the factory phase, before every binding's
/// factory, wherever the bindings fall in `imports = [..]`.
pub type RedisSetup<I = Unnamed> = ConfigSetup<RedisModule<I>, RedisConfig, I>;

impl<I: Instance> Module for RedisModule<I> {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        ConfigModule::provide_feature::<I, RedisConfig>(builder)
            .provide_meta(RedisConnection::<I>::declared_budget())
            .provide_factory_after::<RedisConnection<I>, (I::Of<RedisConfig>,), _, _>(
                |container| async move {
                    let held = ConfigModule::resolved_for::<I, RedisConfig>(&container)?;
                    let config: &RedisConfig = (*held).borrow();
                    Ok(RedisConnection::<I>::open(config).await?)
                },
            )
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        #[cfg(feature = "health")]
        let builder = crate::health::register::<I, Self>(builder);
        builder
    }
}
