//! [`RedisThrottlerModule`] — binds [`RedisThrottler`] as the shared
//! `dyn ThrottlerStore` the `nest-rs-throttler` `ThrottlerGuard` injects, in
//! place of the in-memory default. Enabled by the `throttler` feature.

use std::sync::Arc;

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};
use nest_rs_throttler::{ThrottlerConfig, ThrottlerStore};

use crate::QueueConnection;
use crate::throttler::RedisThrottler;

/// Cross-process rate-limit store. Wire with `RedisThrottlerModule::for_root(None)`
/// **instead of** `ThrottlerModule::for_root(...)` — both supply the same
/// `dyn ThrottlerStore` binding, so import exactly one. The `ThrottlerGuard`
/// binds per route unchanged.
///
/// Reuses the app's Redis connection ([`QueueConnection`]), so
/// [`QueueModule::for_root`](crate::QueueModule::for_root) must be imported
/// **before** this module — its connection is a factory output this module's
/// factory reads. Config is the same `NESTRS_THROTTLER__*` namespace as the
/// in-memory module (one dual-path config surface for both backends).
pub struct RedisThrottlerModule;

impl RedisThrottlerModule {
    /// Pass `None` to load [`ThrottlerConfig`] from `NESTRS_THROTTLER__*`, or a
    /// [`ThrottlerConfig`] to pin it in code (wins over the environment).
    pub fn for_root(config: impl Into<Option<ThrottlerConfig>>) -> RedisThrottlerSetup {
        RedisThrottlerSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by `RedisThrottlerModule::for_root`. Registers
/// the Redis-backed throttler store so rate limits are shared across instances.
pub struct RedisThrottlerSetup {
    pinned: Option<ThrottlerConfig>,
}

impl DynamicModule for RedisThrottlerSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        // Same `Arc<dyn ThrottlerStore>` factory-output binding the in-memory
        // module registers — a factory output so the guard's
        // `#[inject] Arc<dyn ThrottlerStore>` resolves as global infrastructure.
        let builder =
            builder.provide_factory::<Arc<dyn ThrottlerStore>, _, _>(|container| async move {
                let config = container
                    .get::<ThrottlerConfig>()
                    .expect("ThrottlerConfig is resolved by ConfigModule::provide_feature");
                let default = nest_rs_throttler::resolve(&config);
                // Import order is a wiring mistake, so it is a boot error — the
                // same channel `resolve` above already uses — not a panic.
                let conn = container.get::<QueueConnection>().ok_or_else(|| {
                    anyhow::anyhow!(
                        "QueueConnection is not registered — import QueueModule::for_root \
                         before RedisThrottlerModule::for_root, whose store reuses that connection"
                    )
                })?;
                Ok(Arc::new(RedisThrottler::new((*conn).clone(), default))
                    as Arc<dyn ThrottlerStore>)
            });
        // The guard rides with the store on every backend — swapping the store
        // must not also change what a controller has to list in `providers`.
        nest_rs_throttler::provide_guard(builder)
    }
}
