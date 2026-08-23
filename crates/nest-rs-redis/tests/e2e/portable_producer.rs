//! C2: `RedisQueueModule` must bind the **portable** producer name too.
//!
//! `/queue/producing-jobs/` tells a feature to inject `Arc<dyn JobProducer>`,
//! and `/queue/writing-a-driver/` states the dyn binding as the contract every
//! driver owes. The first-party Redis driver used to seed only the concrete
//! type, so the documented portable form compiled and then died at boot with
//! `unmet dependency: dyn JobProducer`. Both names must resolve from one
//! connection — the one `RedisModule::for_root` opens.

use std::sync::Arc;

use nest_rs_core::{App, module};
use nest_rs_queue::JobProducer;
use nest_rs_redis::{RedisModule, RedisQueueModule, RedisQueueProducer};

use crate::redis_config;

#[module(imports = [RedisModule::for_root(redis_config()), RedisQueueModule])]
struct PortableProducerModule;

#[tokio::test]
async fn the_queue_binding_resolves_both_the_concrete_and_the_portable_producer_name() {
    let app = App::builder()
        .module::<PortableProducerModule>()
        .build()
        .await
        .expect("the queue binding boots against the dev-container Redis");

    assert!(
        app.container().get::<RedisQueueProducer>().is_some(),
        "the concrete backend stays injectable",
    );
    let producer: Option<Arc<dyn JobProducer>> = app.container().get_dyn::<dyn JobProducer>();
    let producer = producer.expect(
        "the documented portable form must resolve from the container, not only \
         by hand-coercing the concrete type",
    );

    // A live producer, not an empty registration.
    producer
        .push_json(
            "nest-rs-redis-e2e-portable",
            serde_json::json!({ "probe": true }),
        )
        .await
        .expect("the portable handle pushes onto the same connection");
}
