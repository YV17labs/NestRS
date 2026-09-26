//! [`RedisQueueModule`] — the producer-side binding. A bare import: it owns no
//! config, the connection is [`RedisModule`](crate::RedisModule)'s, and what it
//! adds is the [`JobProducer`] bound over that connection.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_core::{ContainerBuilder, Module};
use nest_rs_queue::JobProducer;

use super::RedisQueueProducer;
use crate::RedisConnection;
use crate::connection::CONNECTION_REMEDY;

/// The producer-side binding. Import it beside
/// [`RedisModule::for_root`](crate::RedisModule::for_root) to push jobs — enough
/// for an API that enqueues without running a consumer.
pub struct RedisQueueModule;

impl Module for RedisQueueModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }

    fn collect(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_collected(TypeId::of::<Self>()) {
            return builder;
        }
        // The concrete backend type is a factory output, so a feature injects it
        // as global infrastructure without importing this module; queued after
        // the connection's factory so `imports` order stays a readability choice.
        //
        // The portable name is the same instance, **declared**: a feature
        // injecting `Arc<dyn JobProducer>` never names the backend, so a second
        // queue backend imported beside this one fails the boot naming both
        // (`BACKEND_REMEDY`) rather than whichever `imports` listed last serving
        // every push in silence — the contract `/queue/writing-a-driver/` asks a
        // driver to honour.
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
                nest_rs_queue::BACKEND_REMEDY,
                |container| async move {
                    let producer = container
                        .get::<RedisQueueProducer>()
                        .ok_or_else(|| anyhow::anyhow!("RedisQueueModule: {CONNECTION_REMEDY}"))?;
                    Ok(producer as Arc<dyn JobProducer>)
                },
            )
    }
}
