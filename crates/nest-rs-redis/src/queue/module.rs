//! [`RedisQueueModule`] — binds the queue port over the shared
//! [`RedisConnection`]: the producer a feature pushes through, and the consumer
//! the port's `QueueWorker` runs jobs with when the app imports
//! `QueueModule`.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule, Module};
use nest_rs_queue::{BACKEND_REMEDY, BoundConsumer, JobProducer};

use super::consumer::RedisQueueConsumer;
use super::{RedisQueueConfig, RedisQueueProducer};
use crate::RedisConnection;
use crate::connection::CONNECTION_REMEDY;

/// The Redis queue binding. Import it beside
/// [`RedisModule::for_root`](crate::RedisModule::for_root): an app pushing jobs
/// injects `Arc<dyn JobProducer>`, and an app running them imports
/// `QueueModule` beside it too. A bare import reads `<PREFIX>_REDIS__QUEUE__*`;
/// [`for_root`](Self::for_root) pins the base those variables overlay.
pub struct RedisQueueModule;

impl RedisQueueModule {
    /// `None` loads [`RedisQueueConfig`] from `<PREFIX>_REDIS__QUEUE__*`;
    /// `Some(cfg)` pins the base those variables overlay, field by field.
    pub fn for_root(config: impl Into<Option<RedisQueueConfig>>) -> RedisQueueSetup {
        RedisQueueSetup {
            pinned: config.into(),
        }
    }

    /// The producer and the consumer, each bound under the port's type and
    /// declared, so a second queue backend imported beside this one fails the
    /// boot naming both ([`BACKEND_REMEDY`]).
    fn bind(builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .provide_factory_after::<RedisQueueProducer, RedisConnection, _, _>(
                |container| async move {
                    let conn = container
                        .get::<RedisConnection>()
                        .ok_or_else(|| anyhow::anyhow!("RedisQueueModule: {CONNECTION_REMEDY}"))?;
                    Ok(RedisQueueProducer::new((*conn).clone()))
                },
            )
            .provide_declared_factory_after::<Arc<dyn JobProducer>, RedisQueueProducer, _, _>(
                BACKEND_REMEDY,
                |container| async move {
                    let producer = container
                        .get::<RedisQueueProducer>()
                        .ok_or_else(|| anyhow::anyhow!("RedisQueueModule: {CONNECTION_REMEDY}"))?;
                    Ok(producer as Arc<dyn JobProducer>)
                },
            )
            .provide_declared_factory_after_both::<BoundConsumer, RedisConnection, RedisQueueConfig, _, _>(
                BACKEND_REMEDY,
                |container| async move {
                    let conn = container
                        .get::<RedisConnection>()
                        .ok_or_else(|| anyhow::anyhow!("RedisQueueModule: {CONNECTION_REMEDY}"))?;
                    let config = container.get::<RedisQueueConfig>().ok_or_else(|| {
                        anyhow::anyhow!("RedisQueueModule: RedisQueueConfig was not resolved")
                    })?;
                    Ok(BoundConsumer::new(RedisQueueConsumer::new(
                        (*conn).clone(),
                        config.lease,
                    )))
                },
            )
    }
}

impl Module for RedisQueueModule {
    // A bare import still reads `<PREFIX>_REDIS__QUEUE__*`; a
    // `for_root(Some(cfg))`'s declared value supersedes this env-only one.
    fn collect(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_collected(TypeId::of::<Self>()) {
            return builder;
        }
        Self::bind(ConfigModule::provide_feature(
            None::<RedisQueueConfig>,
            builder,
        ))
    }

    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }
}

/// The configured import [`RedisQueueModule::for_root`] returns: the config it
/// pins, and the bindings — queued once, however many sites import the module.
pub struct RedisQueueSetup {
    pinned: Option<RedisQueueConfig>,
}

impl DynamicModule for RedisQueueSetup {
    fn module() -> TypeId {
        TypeId::of::<RedisQueueModule>()
    }

    fn collect(&self, mut builder: ContainerBuilder) -> ContainerBuilder {
        let first = builder.mark_collected(TypeId::of::<RedisQueueModule>());
        let builder = ConfigModule::provide_feature(self.pinned.clone(), builder);
        if first {
            RedisQueueModule::bind(builder)
        } else {
            builder
        }
    }
}
