//! [`RedisModule`] — the substrate seam. `RedisModule::for_root(None)` opens
//! the one [`RedisConnection`] this crate's bindings share; each binding
//! (`RedisQueueModule`, `RedisThrottlerModule`,
//! `RedisScheduleModule`) is then a
//! bare import beside it, or its own `for_root` when it owns a config of its
//! own, and reads the connection from the container.
//!
//! This is the crate-root `module.rs` a driver is allowed exactly once: a
//! module *of Redis* — the crate's own subject — and not one binding wearing the
//! crate's name. Every binding folder reaches it, which is the level a shared
//! thing belongs at.

use std::any::TypeId;

use nest_rs_config::ConfigModule;
use nest_rs_core::{Budget, ContainerBuilder, DynamicModule};

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

    fn collect(&self, mut builder: ContainerBuilder) -> ContainerBuilder {
        if builder.mark_collected(TypeId::of::<RedisModule>()) {
            builder = builder.provide_meta(Budget::of::<RedisConnection>(
                "the Redis connection",
                format!(
                    "{}, or `RedisConfig::connect_timeout` in code",
                    nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS"),
                ),
                |conn| Some(conn.budget()),
            ));
        }
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        builder.provide_factory::<RedisConnection, _, _>(|container| async move {
            #[expect(
                clippy::expect_used,
                reason = "provide_feature queued the config's factory in this module's collect"
            )]
            let config = container
                .get::<RedisConfig>()
                .expect("RedisConfig is resolved by ConfigModule::provide_feature");
            // `?` lifts the typed `RedisError` into the factory's `anyhow`
            // boundary (the composition-root error channel).
            Ok(RedisConnection::connect(&config).await?)
        })
    }
}
