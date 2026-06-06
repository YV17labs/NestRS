//! Owns the shared Redis [`QueueConnection`].
//!
//! The connection is async, built in the collect phase before the module tree
//! is wired, so `QueueWorker` and every producer inject it regardless of
//! import order.

use nestrs_config::ConfigModule;
use nestrs_core::{ContainerBuilder, DynamicModule};

use crate::QueueConnection;
use crate::config::QueueConfig;

pub struct QueueModule;

impl QueueModule {
    /// `None` ⇒ load from `NESTRS_QUEUE__*`; `Some(cfg)` pins in code.
    pub fn for_root(config: impl Into<Option<QueueConfig>>) -> QueueSetup {
        QueueSetup {
            pinned: config.into(),
        }
    }
}

pub struct QueueSetup {
    pinned: Option<QueueConfig>,
}

impl DynamicModule for QueueSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        builder.provide_factory::<QueueConnection, _, _>(|container| async move {
            let config = container
                .get::<QueueConfig>()
                .expect("QueueConfig is resolved by ConfigModule::provide_feature");
            QueueConnection::connect(&config.url).await
        })
    }
}
