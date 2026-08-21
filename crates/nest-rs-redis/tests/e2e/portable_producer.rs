//! C2: `RedisQueueModule::for_root` must bind the **portable** producer name too.
//!
//! `/queue/producing-jobs/` tells a feature to inject either
//! `Arc<RedisQueueConnection>` (concrete) or `Arc<dyn JobProducer>` (portable), and
//! `/queue/writing-a-driver/` states the dyn binding as the contract every
//! driver owes. The first-party Redis driver used to seed only the concrete
//! type, so the documented portable form compiled and then died at boot with
//! `unmet dependency: dyn JobProducer`. Both names must resolve from one
//! connection.

use std::sync::Arc;

use nest_rs_core::{App, module};
use nest_rs_queue::JobProducer;
use nest_rs_redis::{RedisQueueConfig, RedisQueueConnection, RedisQueueModule};

use crate::redis_url;

/// Pinned rather than env-sourced: the workspace forbids `unsafe`, so a test
/// cannot publish `NESTRS_QUEUE__URL` into the process env.
fn dev_container_queue() -> RedisQueueConfig {
    RedisQueueConfig {
        url: redis_url(),
        ..Default::default()
    }
}

#[module(imports = [RedisQueueModule::for_root(dev_container_queue())])]
struct PortableProducerModule;

#[tokio::test]
async fn for_root_binds_both_the_concrete_and_the_portable_producer_name() {
    let app = App::builder()
        .module::<PortableProducerModule>()
        .build()
        .await
        .expect("the queue module boots against the dev-container Redis");

    assert!(
        app.container().get::<RedisQueueConnection>().is_some(),
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
