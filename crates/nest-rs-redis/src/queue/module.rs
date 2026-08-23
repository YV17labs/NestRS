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
        // Bound as both names: `Arc<RedisQueueProducer>` for the concrete
        // backend and `Arc<dyn JobProducer>` for the portable form the queue
        // docs prescribe — the contract `/queue/writing-a-driver/` asks a driver
        // to honour. A factory output so a producing feature injects it as
        // global infrastructure without importing this module; queued *after*
        // the connection's factory so `imports` order stays a readability
        // choice.
        builder.provide_factory_dyn_after::<RedisQueueProducer, dyn JobProducer, RedisConnection, _, _>(
            |container| async move {
                let conn = container
                    .get::<RedisConnection>()
                    .ok_or_else(|| anyhow::anyhow!("RedisQueueModule: {CONNECTION_REMEDY}"))?;
                Ok(RedisQueueProducer::new((*conn).clone()))
            },
            |producer| Arc::new(producer) as Arc<dyn JobProducer>,
        )
    }
}
