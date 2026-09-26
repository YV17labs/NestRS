//! The producer moving its delayed jobs onto their queue, with no worker
//! running.
//!
//! A delayed job waits on the queue's schedule, and only a scan moves it onto
//! the list a worker fetches from — the list an autoscaler reads. A deployment
//! scaled to zero runs no worker to scan, so the producer that filed the job
//! scans until it is due: the job appears on the list on time, and the
//! autoscaler reading it can start the worker that runs it.

use std::time::Duration;

use nest_rs_core::module;
use nest_rs_queue::{JobProducerExt, PushOptions};
use nest_rs_redis::{RedisModule, RedisQueueModule, RedisQueueProducer};
use nest_rs_testing::TestApp;

/// How long the job is held back.
const DELAY: Duration = Duration::from_secs(1);

#[module(imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule])]
struct ProducerOnlyModule;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delayed_push_reaches_its_queue_when_due_with_no_worker_running() {
    let app = TestApp::builder()
        .module::<ProducerOnlyModule>()
        .build_headless()
        .await
        .expect("a producer-only app boots");
    let producer = app
        .container()
        .get::<RedisQueueProducer>()
        .expect("the producer binding");
    // A queue of this run's own: nothing drains it, so an earlier run's jobs
    // would still be there.
    let queue = format!("nestrs-e2e-promoter-{}", crate::this_run());
    producer
        .push_json(
            &queue,
            serde_json::json!({ "probe": true }),
            PushOptions::default().with_delay(DELAY),
        )
        .await
        .expect("a delayed push");
    assert_eq!(
        crate::waiting(&queue).await,
        0,
        "held back, the job is not on the queue yet"
    );

    let mut on_the_queue = 0;
    for _ in 0..60 {
        on_the_queue = crate::waiting(&queue).await;
        if on_the_queue > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        on_the_queue, 1,
        "once due, the producer moved it onto the list a worker — and an autoscaler — reads",
    );

    let mut admin = crate::connect().await;
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{}:*", crate::namespace(&queue)))
        .query_async(&mut admin)
        .await
        .expect("KEYS");
    let _: i64 = redis::cmd("DEL")
        .arg(&keys)
        .query_async(&mut admin)
        .await
        .expect("DEL");
}
