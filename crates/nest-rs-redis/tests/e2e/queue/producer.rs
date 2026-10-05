//! The push surface against a live Redis: a push held back until Redis's clock
//! says it is due, a push under a unique key, and a cancel — each one script.
//!
//! A job held back waits off the stream, in the queue's `due` set with its
//! record beside it, and reaches the stream only once due; a unique key is
//! claimed in the script that files the job, so a second push under it is
//! refused naming the job that holds it and files nothing, over any number of
//! racing pushes. A cancel is proved here where no worker runs, so nothing can
//! take the job first; what a running job does with a cancel is the consumer's.
//!
//! Every test pushing where no worker drains files under a name of its own and
//! forgets what it filed. And a push through a Redis gone silent fails within
//! one connection budget, with the connection's own cause, so the port's net
//! never answers first.

use std::time::{Duration, Instant};

use nest_rs_queue::{
    JobId, JobProducer, JobProducerExt, PushOptions, PushReceipt, QueueError, QueueName,
};
use nest_rs_redis::{RedisConfig, RedisConnection, RedisQueueProducer};
use serde_json::json;

/// The holder a refused push names, or a panic saying what came back instead.
fn holder_of(refused: QueueError) -> JobId {
    match refused {
        QueueError::UniqueKeyHeld { holder, .. } => holder,
        other => panic!("a push under a held key is refused as held, not {other:?}"),
    }
}

/// When the job `job` of `queue` is due, in Redis's milliseconds, if it is
/// held back.
async fn due_of(queue: &str, job: &JobId) -> Option<u64> {
    redis::cmd("ZSCORE")
        .arg(crate::key_of(queue, "due"))
        .arg(job.to_string())
        .query_async(&mut crate::connect().await)
        .await
        .expect("ZSCORE")
}

/// Redis's clock, in milliseconds.
async fn redis_now() -> u64 {
    let (seconds, micros): (u64, u64) = redis::cmd("TIME")
        .query_async(&mut crate::connect().await)
        .await
        .expect("TIME");
    seconds * 1000 + micros / 1000
}

/// A push held back waits off the stream — its record in `delayed`, due in
/// `due` at the instant Redis's clock gives it — and an immediate push lands on
/// the stream, indexed by its job.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delayed_push_waits_off_the_stream_until_redis_says_it_is_due() {
    let producer = crate::producer().await;
    let queue = format!("nestrs-e2e-producer-delay-{}", crate::this_run());
    let before = redis_now().await;
    let held = producer
        .push_json(
            &queue,
            json!({ "take": 1 }),
            PushOptions::default().with_delay(Duration::from_secs(60)),
        )
        .await
        .expect("a delayed push");
    let after = redis_now().await;

    let due = due_of(&queue, held.id()).await.expect("held back");
    assert!(
        (before + 60_000..=after + 60_000).contains(&due),
        "due a minute after the push, on Redis's clock: {due} not in {before}..{after} + 60s",
    );
    assert!(
        crate::field_of(&queue, "delayed", &held.id().to_string())
            .await
            .is_some(),
        "its record waits beside its due instant",
    );
    assert_eq!(
        crate::filed(&queue).await,
        0,
        "and nothing is on the stream"
    );

    let now = producer
        .push_json(&queue, json!({ "take": 2 }), None)
        .await
        .expect("an immediate push");
    assert_eq!(
        crate::filed(&queue).await,
        1,
        "an immediate push is on the stream"
    );
    assert!(
        crate::field_of(&queue, "entries", &now.id().to_string())
            .await
            .is_some(),
        "indexed by its job, for a cancel to find",
    );
    crate::forget(&queue).await;
}

/// A push under a key another job on the queue holds is refused, naming that
/// job, and files nothing; the same key on another queue names another job.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_unique_push_under_a_held_key_is_refused_naming_the_holder_and_files_nothing() {
    let producer = crate::producer().await;
    let queue = format!("nestrs-e2e-unique-held-{}", crate::this_run());
    let elsewhere = format!("{queue}-elsewhere");
    let under = || PushOptions::default().with_unique("song-1");

    let first = producer
        .push_json(&queue, json!({ "take": 1 }), under())
        .await
        .expect("the first push takes the key");
    let refused = producer
        .push_json(&queue, json!({ "take": 2 }), under())
        .await
        .expect_err("the key is held");
    let message = refused.to_string();
    assert_eq!(
        holder_of(refused),
        *first.id(),
        "the refusal names the holder"
    );
    assert!(
        message.contains("song-1") && message.contains(&queue),
        "{message}"
    );
    assert_eq!(
        crate::filed(&queue).await,
        1,
        "the refused push filed nothing"
    );

    producer
        .push_json(&elsewhere, json!({ "take": 1 }), under())
        .await
        .expect("a key is scoped to its queue");

    crate::forget(&queue).await;
    crate::forget(&elsewhere).await;
}

/// Pushes racing for one key file one job: the claim is read and taken in the
/// script that files it, so no two see it free.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unique_push_racing_for_one_key_files_exactly_one_job() {
    let producer = crate::producer().await;
    let queue = format!("nestrs-e2e-unique-race-{}", crate::this_run());
    let pushes = (0..16).map(|take| {
        let producer = producer.clone();
        let queue = queue.clone();
        tokio::spawn(async move {
            producer
                .push_json(
                    &queue,
                    json!({ "take": take }),
                    PushOptions::default().with_unique("the-one"),
                )
                .await
        })
    });
    let mut filed = Vec::new();
    let mut holders = Vec::new();
    for push in pushes {
        match push.await.expect("the push task") {
            Ok(receipt) => filed.push(receipt),
            Err(refused) => holders.push(holder_of(refused)),
        }
    }

    assert_eq!(filed.len(), 1, "one push took the key");
    assert!(
        holders.iter().all(|holder| holder == filed[0].id()),
        "every other push was refused naming it: {holders:?}",
    );
    assert_eq!(crate::filed(&queue).await, 1, "and one job was filed");
    crate::forget(&queue).await;
}

/// A job no worker has taken is cancelled — `true`, once — whether it waits on
/// the stream or held back, and leaves no record behind; a job the backend
/// never knew, and a key no job holds, answer `false`. A cancel names a job by
/// its receipt, which a caller may keep as data and rebuild.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_answers_true_once_for_a_waiting_job_and_leaves_nothing() {
    let producer = crate::producer().await;
    let queue = format!("nestrs-e2e-cancel-waiting-{}", crate::this_run());
    let waiting = producer
        .push_json(
            &queue,
            json!({ "take": 1 }),
            PushOptions::default().with_unique("song-1"),
        )
        .await
        .expect("a push nothing drains");
    let held = producer
        .push_json(
            &queue,
            json!({ "take": 2 }),
            PushOptions::default()
                .with_delay(Duration::from_secs(60))
                .with_unique("song-2"),
        )
        .await
        .expect("a delayed push nothing drains");
    let kept: PushReceipt = serde_json::from_value(serde_json::to_value(&waiting).expect("data"))
        .expect("a receipt read back");

    assert!(
        producer.cancel(&kept).await.expect("a cancel"),
        "the job waited, so it never starts"
    );
    assert!(
        !producer.cancel(&kept).await.expect("a second cancel"),
        "a cancelled job is not cancelled twice",
    );
    let name = QueueName::new(queue.clone()).expect("a valid name");
    assert!(
        producer
            .remove_unique(&name, "song-2")
            .await
            .expect("a cancel by key"),
        "a job held back is cancelled by the key it holds",
    );
    assert!(
        !producer
            .remove_unique(&name, "song-2")
            .await
            .expect("a second cancel by key"),
        "and its key is free",
    );
    assert_eq!(crate::filed(&queue).await, 0, "nothing waits on the stream");
    assert_eq!(due_of(&queue, held.id()).await, None, "nor held back");
    assert_eq!(
        crate::keys_of(&queue).await,
        [crate::key_of(&queue, "jobs")],
        "and no record of either job is left: only the stream they waited on",
    );

    let stranger = PushReceipt::new(
        QueueName::new(queue.clone()).expect("a valid name"),
        JobId::parse(&uuid::Uuid::now_v7().to_string()).expect("a job id"),
    );
    assert!(
        !producer.cancel(&stranger).await.expect("a cancel"),
        "a job the backend never knew is not cancelled",
    );
    crate::forget(&queue).await;
}

/// A push through a Redis gone silent fails at the connection's budget as the
/// connection's own failure, naming the variable that sets it — within one
/// budget, before the port's net could answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_push_through_a_silent_redis_fails_within_one_budget_naming_it() {
    let proxy = crate::DarkeningProxy::start().await;
    let budget = Duration::from_secs(1);
    let mut conn = RedisConnection::connect(&RedisConfig {
        url: proxy.url(),
        connect_timeout: budget,
        ..RedisConfig::default()
    })
    .await
    .expect("connect through the proxy");
    let producer = RedisQueueProducer::new(conn.clone());

    proxy.go_dark();
    // The first command to fail is where the client starts reopening the
    // connection; from there, every command waits on one nothing answers.
    let mut saw_the_drop = false;
    for _ in 0..50 {
        if redis::cmd("PING")
            .query_async::<()>(&mut conn)
            .await
            .is_err()
        {
            saw_the_drop = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        saw_the_drop,
        "the proxy going dark must drop the connection"
    );

    let queue = format!("nestrs-e2e-silent-{}", crate::this_run());
    let started = Instant::now();
    let refused = producer
        .push_json(&queue, json!({ "probe": true }), None)
        .await
        .expect_err("nothing answers once the proxy is dark");
    let took = started.elapsed();

    assert!(
        matches!(refused, QueueError::Backend(_)),
        "the connection's own failure, not the port's net: {refused:?}"
    );
    let cause = nest_rs_core::error_message(&refused);
    assert!(
        cause.contains(&nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS")),
        "{cause}"
    );
    assert!(took < budget * 3 / 2, "one budget: took {took:?}");
}
