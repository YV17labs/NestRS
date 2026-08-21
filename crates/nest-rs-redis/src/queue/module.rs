//! Owns the shared Redis [`RedisQueueConnection`].
//!
//! The connection is async, built in the collect phase before the module tree
//! is wired, so `RedisWorker` and every producer inject it regardless of
//! import order.

use std::sync::Arc;

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};
use nest_rs_queue::JobProducer;

use super::RedisQueueConfig;
use super::RedisQueueConnection;

/// The producer-side activation seam. Import [`RedisQueueModule::for_root`] to build
/// and share the Redis [`RedisQueueConnection`](crate::RedisQueueConnection) — enough to
/// push jobs without running a consumer.
pub struct RedisQueueModule;

impl RedisQueueModule {
    /// `None` ⇒ load from `NESTRS_QUEUE__*`; `Some(cfg)` pins in code.
    pub fn for_root(config: impl Into<Option<RedisQueueConfig>>) -> RedisQueueSetup {
        RedisQueueSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`RedisQueueModule::for_root`]. Builds the Redis
/// connection in the collect phase and registers it as [`RedisQueueConnection`](crate::RedisQueueConnection).
pub struct RedisQueueSetup {
    pinned: Option<RedisQueueConfig>,
}

impl DynamicModule for RedisQueueSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        // Bound as both names: `Arc<RedisQueueConnection>` for the concrete backend
        // and `Arc<dyn JobProducer>` for the portable form the queue docs
        // prescribe — the very contract `/queue/writing-a-driver/` asks a driver
        // to honour. `RedisQueueConnection`'s `Clone` is a handle clone over the one
        // multiplexed connection, so both names share a single socket.
        builder.provide_factory_dyn::<RedisQueueConnection, dyn JobProducer, _, _>(
            |container| async move {
                let config = container
                    .get::<RedisQueueConfig>()
                    .expect("RedisQueueConfig is resolved by ConfigModule::provide_feature");
                // `?` lifts the typed `RedisError` into the factory's `anyhow`
                // boundary (the composition-root error channel).
                Ok(
                    RedisQueueConnection::connect_within(&config.url, config.connect_timeout)
                        .await?,
                )
            },
            |conn| Arc::new(conn) as Arc<dyn JobProducer>,
        )
    }
}
