//! [`RedisModule`] — the substrate seam. `RedisModule::for_root(None)` opens
//! the one [`RedisConnection`] this crate's bindings share; each binding
//! (`RedisQueueModule`, `RedisThrottlerModule`, `RedisScheduleModule`) is then a
//! bare import beside it, or its own `for_root` when it owns a config.

use std::any::TypeId;

use nest_rs_config::ConfigModule;
use nest_rs_core::{Collecting, ContainerBuilder, DynamicModule};

use crate::{RedisConfig, RedisConnection};

/// The Redis substrate. Import [`RedisModule::for_root`] once, then the
/// bindings your app needs beside it — they share the connection it opens.
pub struct RedisModule;

impl RedisModule {
    /// `None` ⇒ load from `<PREFIX>_REDIS__*`; `Some(cfg)` pins the base those
    /// variables overlay, per field.
    pub fn for_root(config: impl Into<Option<RedisConfig>>) -> RedisSetup {
        RedisSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`RedisModule::for_root`]. Resolves
/// [`RedisConfig`] and opens the [`RedisConnection`] in the collect phase, so
/// every binding's factory — wherever it falls in `imports = [..]` — finds the
/// connection already built.
pub struct RedisSetup {
    pinned: Option<RedisConfig>,
}

impl DynamicModule for RedisSetup {
    fn module() -> TypeId {
        TypeId::of::<RedisModule>()
    }

    fn collect(&self, builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(
            self.pinned.clone(),
            builder.provide_meta(RedisConnection::declared_budget()),
        );
        builder.provide_factory_after::<RedisConnection, (RedisConfig,), _, _>(
            |container| async move {
                let config = ConfigModule::resolved::<RedisConfig>(&container)?;
                Ok(RedisConnection::connect(&config).await?)
            },
        )
    }
}
