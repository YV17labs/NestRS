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
use nest_rs_redis::{RedisConnection, RedisModule, RedisQueueModule, RedisQueueProducer};

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
    let queue = "nest-rs-redis-e2e-portable";
    producer
        .push_json(queue, serde_json::json!({ "probe": true }))
        .await
        .expect("the portable handle pushes onto the same connection");

    // Nothing drains this queue, so what the test pushed is removed rather
    // than left in the shared Redis, one record per run.
    let conn = app
        .container()
        .get::<RedisConnection>()
        .expect("RedisModule opened the pool");
    let mut pooled = conn.connection().await.expect("a pooled connection");
    let pending = format!("nestrs:queue:queue:{queue}");
    let ids: Vec<String> = redis::cmd("LRANGE")
        .arg(&pending)
        .arg(0)
        .arg(-1)
        .query_async(&mut pooled)
        .await
        .expect("read the pushed ids");
    assert!(
        !ids.is_empty(),
        "the push landed on the queue's pending list"
    );
    redis::cmd("HDEL")
        .arg("nestrs:queue:jobs")
        .arg(&ids)
        .query_async::<()>(&mut pooled)
        .await
        .expect("remove the pushed records");
    redis::cmd("DEL")
        .arg(&pending)
        .query_async::<()>(&mut pooled)
        .await
        .expect("remove the pending list");
}

/// The raw hatch takes any string. A name under the bindings' own key prefix
/// used to be written verbatim — `nestrs:queue:dead` onto the dead list itself —
/// and reported as a successful push.
#[tokio::test]
async fn a_queue_name_under_the_bindings_own_key_prefix_is_refused() {
    let producer = RedisQueueProducer::new(crate::connect().await);
    let error = producer
        .push_json("nestrs:queue:dead", serde_json::json!({ "probe": true }))
        .await
        .expect_err("a reserved queue name is refused, not written");
    let cause = std::error::Error::source(&error)
        .map(ToString::to_string)
        .unwrap_or_default();
    assert!(
        cause.contains("nestrs:queue:dead"),
        "the refusal names the queue: {error} / {cause}",
    );
}
