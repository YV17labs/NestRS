//! `RedisQueueModule` binds the **portable** names: `/queue/producing-jobs/`
//! tells a feature to inject `Arc<dyn JobProducer>`, and the port's worker runs
//! the `BoundConsumer` — both from the one connection `RedisModule::for_root`
//! opens, whether the binding is a bare import or a `for_root`.

use std::sync::Arc;

use nest_rs_core::{App, module};
use nest_rs_queue::{BoundConsumer, JobProducer, JobProducerExt};
use nest_rs_redis::{RedisModule, RedisQueueConfig, RedisQueueModule, RedisQueueProducer};

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
    let receipt = producer
        .push_json(
            "nest-rs-redis-e2e-portable",
            serde_json::json!({ "probe": true }),
            None,
        )
        .await
        .expect("the portable handle pushes onto the same connection");
    assert_eq!(receipt.queue().as_str(), "nest-rs-redis-e2e-portable");
}

#[module(imports = [
    RedisModule::for_root(redis_config()),
    RedisQueueModule,
    RedisQueueModule::for_root(RedisQueueConfig { lease: std::time::Duration::from_secs(7) }),
])]
struct PinnedAndBareModule;

/// A bare import and a `for_root` of the binding bind one producer and one
/// consumer — the pin's config, never a contest between two bindings.
#[tokio::test]
async fn a_bare_import_beside_a_for_root_binds_once_with_the_pinned_config() {
    let app = App::builder()
        .module::<PinnedAndBareModule>()
        .build()
        .await
        .expect("the two import sites boot as one binding");
    assert!(app.container().get::<BoundConsumer>().is_some());
    assert_eq!(
        app.container()
            .get::<RedisQueueConfig>()
            .expect("the binding's config")
            .lease,
        std::time::Duration::from_secs(7),
    );
}
