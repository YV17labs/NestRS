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

    // A live producer, not an empty registration — on a queue nothing drains,
    // so it is named for this run and deleted.
    let queue = format!("nestrs-e2e-portable-{}", crate::this_run());
    let receipt = producer
        .push_json(&queue, serde_json::json!({ "probe": true }), None)
        .await
        .expect("the portable handle pushes onto the same connection");
    assert_eq!(receipt.queue().as_str(), queue);
    crate::forget(&queue).await;
    assert_eq!(
        crate::filed(&queue).await,
        0,
        "the probe left nothing behind"
    );
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

// The lease leaves a renewal room under the budget, so the net is the one
// refusal: the lease's own is the consumer factory's, and runs first.
#[module(imports = [
    RedisModule::for_root(nest_rs_redis::RedisConfig {
        connect_timeout: nest_rs_queue::BACKEND_TIMEOUT,
        ..redis_config()
    }),
    RedisQueueModule::for_root(RedisQueueConfig { lease: std::time::Duration::from_secs(31) }),
])]
struct PatientProducerModule;

/// A budget at the queue port's net would let the port give up on a push still
/// answering, and fail it without its cause: the binding declares the net over
/// the connection, so the boot refuses it — a producer-only app included —
/// naming both durations and the variable.
#[tokio::test]
async fn a_budget_at_the_queue_ports_net_fails_the_boot() {
    let Err(refused) = App::builder()
        .module::<PatientProducerModule>()
        .build()
        .await
    else {
        panic!("a budget at the queue port's net must not boot");
    };
    let refused = crate::budget_past_net(refused);
    assert_eq!(
        (refused.resource, refused.port, refused.budget, refused.net),
        (
            "the Redis connection",
            "the queue port",
            nest_rs_queue::BACKEND_TIMEOUT,
            nest_rs_queue::BACKEND_TIMEOUT
        )
    );
}

#[module(imports = [
    RedisModule::for_root(nest_rs_redis::RedisConfig {
        connect_timeout: std::time::Duration::from_secs(2),
        ..redis_config()
    }),
    RedisQueueModule::for_root(RedisQueueConfig { lease: std::time::Duration::from_secs(3) }),
])]
struct UnrenewableLeaseModule;

/// A renewal is sent a third into the lease and may wait out the budget, so a
/// lease of no more than one and a half budgets can lapse while Redis still
/// answers: the binding refuses it at boot, naming both variables.
#[tokio::test]
async fn a_lease_a_renewal_cannot_fit_in_fails_the_boot() {
    let Err(refused) = App::builder()
        .module::<UnrenewableLeaseModule>()
        .build()
        .await
    else {
        panic!("a lease a renewal cannot fit in must not boot");
    };
    let said = format!("{refused:#}");
    assert!(
        said.contains(&nest_rs_config::var_name("redis__queue", "LEASE_SECS"))
            && said.contains(&nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS")),
        "{said}"
    );
}
