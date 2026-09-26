//! The producer moving its delayed jobs onto their queue, with no worker
//! running.
//!
//! A delayed job waits on the queue's schedule, and only a scan moves it onto
//! the list a worker fetches from — the list an autoscaler reads. A deployment
//! scaled to zero runs no worker to scan, so the producer that filed the job
//! scans until it is due: the job appears on the list on time, and the
//! autoscaler reading it can start the worker that runs it.

use std::time::Duration;

use nest_rs_queue::{JobProducerExt, PushOptions};

/// How long the job is held back.
const DELAY: Duration = Duration::from_secs(1);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delayed_push_reaches_its_queue_when_due_with_no_worker_running() {
    let producer = crate::producer().await;
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

    crate::forget(&queue).await;
}
