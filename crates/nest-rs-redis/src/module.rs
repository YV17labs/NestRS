//! Owns the shared Redis [`QueueConnection`].
//!
//! The connection is async, built in the collect phase before the module tree
//! is wired, so `QueueWorker` and every producer inject it regardless of
//! import order.

use std::sync::Arc;

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule};
use nest_rs_queue::JobProducer;

use crate::QueueConnection;
use crate::config::QueueConfig;

/// The producer-side activation seam. Import [`QueueModule::for_root`] to build
/// and share the Redis [`QueueConnection`](crate::QueueConnection) — enough to
/// push jobs without running a consumer.
pub struct QueueModule;

impl QueueModule {
    /// `None` ⇒ load from `NESTRS_QUEUE__*`; `Some(cfg)` pins in code.
    pub fn for_root(config: impl Into<Option<QueueConfig>>) -> QueueSetup {
        QueueSetup {
            pinned: config.into(),
        }
    }
}

/// The configured import produced by [`QueueModule::for_root`]. Builds the Redis
/// connection in the collect phase and registers it as [`QueueConnection`](crate::QueueConnection).
pub struct QueueSetup {
    pinned: Option<QueueConfig>,
}

impl DynamicModule for QueueSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        // Bound as both names: `Arc<QueueConnection>` for the concrete backend
        // and `Arc<dyn JobProducer>` for the portable form the queue docs
        // prescribe — the very contract `/queue/writing-a-driver/` asks a driver
        // to honour. `QueueConnection`'s `Clone` is a handle clone over the one
        // multiplexed connection, so both names share a single socket.
        builder.provide_factory_dyn::<QueueConnection, dyn JobProducer, _, _>(
            |container| async move {
                let config = container
                    .get::<QueueConfig>()
                    .expect("QueueConfig is resolved by ConfigModule::provide_feature");
                // `?` lifts the typed `RedisError` into the factory's `anyhow`
                // boundary (the composition-root error channel).
                Ok(QueueConnection::connect_within(&config.url, config.connect_timeout).await?)
            },
            |conn| Arc::new(conn) as Arc<dyn JobProducer>,
        )
    }
}
