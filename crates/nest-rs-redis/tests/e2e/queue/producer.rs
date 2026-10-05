//! The push surface against a live Redis: a push held back until its delay
//! ends, a push under a unique key, and a cancel.
//!
//! The backend declares delayed delivery, so a delayed push is filed on the
//! queue's schedule rather than its list, due on the second its delay ends —
//! rounded up, since apalis schedules on whole seconds. A job must therefore
//! never start before its delay has passed, and must start soon after: the
//! worker scans the schedule every second.
//!
//! A unique key is claimed before the job is filed, so a second push under it is
//! refused naming the job that holds it and files nothing — once over any number
//! of racing pushes — and a key whose job vanished holds out until a cancel frees
//! it or its bound lapses, never forever. A cancel is proved here where no
//! worker runs, so nothing can take the job first; what a worker does with a
//! cancelled job is the delivery guard's, in `worker/lease.rs`.
//!
//! Every test pushing where no worker drains files under a name of its own, or
//! under a key of its own, and forgets what it filed.
//!
//! And a push through a Redis gone silent fails within one connection budget,
//! with the connection's own cause, so the port's net never answers first.

use std::time::{Duration, Instant};

use nest_rs_core::{injectable, module};
use nest_rs_queue::{
    JobId, JobProducerExt, PushOptions, PushReceipt, QueueError, QueueName, processor, queue,
};
use nest_rs_redis::{
    RedisConfig, RedisConnection, RedisModule, RedisQueueModule, RedisQueueProducer,
    RedisWorkerModule,
};
use nest_rs_testing::TestApp;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{DB_UNIQUE_REFUSED, Runs};

/// How long the job is held back.
const DELAY: Duration = Duration::from_secs(2);

/// The latest it may start after its delay: the second the delay is rounded up
/// to, one scan of the schedule, and a poll of the queue.
const LATENESS: Duration = Duration::from_millis(2500);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DelayedCommand {
    run: u64,
}

static DELAYED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-producer-delay", job = DelayedCommand)]
struct DelayedQueue;

#[injectable]
#[derive(Default)]
struct DelayedProcessor;

#[processor]
impl DelayedProcessor {
    #[process(queue = DelayedQueue, retries = 0)]
    async fn run(&self, job: DelayedCommand) -> anyhow::Result<()> {
        DELAYED.start(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [DelayedProcessor],
)]
struct DelayedModule;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delayed_push_runs_once_its_delay_has_passed_and_not_before() {
    let run = crate::this_run();
    let replica = crate::replica::<DelayedModule>().await;
    let pushed = Instant::now();
    replica
        .producer
        .push(
            DelayedQueue,
            DelayedCommand { run },
            PushOptions::default().with_delay(DELAY),
        )
        .await
        .expect("a delayed push");

    crate::wait_until(DELAY + LATENESS * 2, || !DELAYED.of(run).is_empty()).await;
    replica.worker.shutdown().await.expect("clean shutdown");

    let started = DELAYED.of(run);
    assert_eq!(started.len(), 1, "the job ran, once");
    let waited = started[0] - pushed;
    assert!(waited >= DELAY, "it waited out its delay, not {waited:?}");
    assert!(
        waited < DELAY + LATENESS,
        "and ran soon after it ended, not {waited:?} after the push"
    );
}

// --- unique jobs ---------------------------------------------------------------------

/// The holder a refused push names, or a panic saying what came back instead.
fn holder_of(refused: QueueError) -> JobId {
    match refused {
        QueueError::UniqueKeyHeld { holder, .. } => holder,
        other => panic!("a push under a held key is refused as held, not {other:?}"),
    }
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
        crate::waiting(&queue).await,
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

/// Pushes racing for one key file one job: the claim is read and taken in one
/// step, so no two see it free.
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
    assert_eq!(crate::waiting(&queue).await, 1, "and one job was filed");
    crate::forget(&queue).await;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HeldCommand {
    take: u64,
}

#[queue(name = "nestrs-e2e-unique-vanished", job = HeldCommand)]
struct VanishedQueue;

/// A job lost outside the framework — its record gone from apalis's structures,
/// as a producer stopped between claiming its key and filing it leaves nothing
/// there — cannot block its key forever. The claim is kept for the documented
/// bound past the job's due time, and until it lapses a push under the key is
/// refused naming the lost job, whose cancel frees it at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_unique_push_whose_job_vanished_holds_its_key_until_cancelled_or_its_bound_lapses() {
    /// The bound the queue documentation states: a week past the job's due time.
    const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;
    let producer = crate::producer().await;
    let queue = "nestrs-e2e-unique-vanished";
    let key = format!("vanished-{}", crate::this_run());
    let claim = crate::key_of(queue, "unique", &key);
    let delay = Duration::from_secs(60);
    let under = || PushOptions::default().with_unique(key.as_str());

    let lost = producer
        .push(
            VanishedQueue,
            HeldCommand { take: 1 },
            under().with_delay(delay),
        )
        .await
        .expect("a delayed push under the key");
    assert_eq!(crate::read(&claim).await, Some(lost.id().to_string()));
    let kept = crate::pttl(&claim).await;
    assert!(
        kept > WEEK_MS + 55_000 && kept <= WEEK_MS + 60_000,
        "the claim is kept a week past the job's due time, not {kept} ms",
    );

    // The job vanishes from the queue storage: apalis's schedule and its record.
    vanish(queue).await;
    let refused = producer
        .push(VanishedQueue, HeldCommand { take: 2 }, under())
        .await
        .expect_err("the lost job still holds the key");
    assert_eq!(holder_of(refused), *lost.id());
    assert!(
        producer
            .cancel_unique(VanishedQueue, &key)
            .await
            .expect("a cancel by key"),
        "the lost job never started, and now never will",
    );
    let next = producer
        .push(VanishedQueue, HeldCommand { take: 3 }, under())
        .await
        .expect("the cancel freed the key");

    // Lost again, and nobody cancels: the bound passes, played by moving the
    // claim's expiry to now.
    vanish(queue).await;
    assert_eq!(crate::read(&claim).await, Some(next.id().to_string()));
    let _: i64 = redis::cmd("PEXPIRE")
        .arg(&claim)
        .arg(1)
        .query_async(&mut crate::connect().await)
        .await
        .expect("PEXPIRE");
    tokio::time::sleep(Duration::from_millis(20)).await;
    let after = producer
        .push(VanishedQueue, HeldCommand { take: 4 }, under())
        .await
        .expect("a claim whose bound lapsed holds nothing");
    assert_eq!(crate::read(&claim).await, Some(after.id().to_string()));

    crate::forget(queue).await;
}

/// Remove `queue`'s jobs from apalis's own structures — the record and where it
/// waits — the way a job lost outside the framework is gone.
async fn vanish(queue: &str) {
    let mut admin = crate::connect().await;
    let _: i64 = redis::cmd("DEL")
        .arg(format!("{}:scheduled", crate::namespace(queue)))
        .arg(format!("{}:active", crate::namespace(queue)))
        .arg(format!("{}:data", crate::namespace(queue)))
        .query_async(&mut admin)
        .await
        .expect("DEL");
}

/// The ACL user the refused-filing test creates, and removes.
const FILING_REFUSED_USER: &str = "nestrs-e2e-filing-refused";

/// Its password — a test fixture, never a secret.
const FILING_REFUSED_PASSWORD: &str = "claims-but-cannot-file";

/// A push whose filing Redis refuses — it answered, so the job is not on the
/// queue — lets go of the key it claimed at once: a push under it right after
/// takes it. Played with a user whose ACL reaches the queue's claims and open
/// records, and none of apalis's structures.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_unique_push_redis_refuses_to_file_frees_its_key_at_once() {
    let queue = format!("nestrs-e2e-unique-refused-{}", crate::this_run());
    let admin_config = crate::redis_config_on(DB_UNIQUE_REFUSED);
    let mut admin = nest_rs_redis::RedisConnection::connect(&admin_config)
        .await
        .expect("the admin connection");
    let _: () = redis::cmd("ACL")
        .arg("SETUSER")
        .arg(FILING_REFUSED_USER)
        .arg("reset")
        .arg("on")
        .arg(format!(">{FILING_REFUSED_PASSWORD}"))
        .arg(format!("~{}", crate::key_of(&queue, "unique", "*")))
        .arg(format!("~{}", crate::key_of(&queue, "open", "*")))
        .arg("+@all")
        .arg("-@dangerous")
        .query_async(&mut admin)
        .await
        .expect("ACL SETUSER");
    let confined = RedisConfig {
        url: crate::redis_url_on(DB_UNIQUE_REFUSED).replacen(
            "://",
            &format!("://{FILING_REFUSED_USER}:{FILING_REFUSED_PASSWORD}@"),
            1,
        ),
        ..Default::default()
    };
    let refusing = producer_on(confined).await;
    let accepting = producer_on(admin_config).await;
    let under = || PushOptions::default().with_unique("song-1");

    let refused = refusing
        .push_json(&queue, json!({ "take": 1 }), under())
        .await
        .expect_err("Redis refuses the filing");
    assert!(
        !matches!(refused, QueueError::UniqueKeyHeld { .. }),
        "the key was free: {refused:?}",
    );
    let taken = accepting
        .push_json(&queue, json!({ "take": 2 }), under())
        .await
        .expect("the refused push let go of the key");
    let _: () = redis::cmd("ACL")
        .arg("DELUSER")
        .arg(FILING_REFUSED_USER)
        .query_async(&mut admin)
        .await
        .expect("ACL DELUSER");

    let claim = crate::key_of(&queue, "unique", "song-1");
    let held: Option<String> = redis::cmd("GET")
        .arg(&claim)
        .query_async(&mut admin)
        .await
        .expect("GET");
    assert_eq!(held, Some(taken.id().to_string()));
    let _: () = redis::cmd("FLUSHDB")
        .query_async(&mut admin)
        .await
        .expect("FLUSHDB");
}

#[module(imports = [RedisModule::for_root(None), RedisQueueModule])]
struct SeededProducerModule;

/// A producer-only app reaching Redis as `redis` says, whatever the environment
/// says — the config seeded, the hermetic-test hatch.
async fn producer_on(redis: RedisConfig) -> RedisQueueProducer {
    let app = TestApp::builder()
        .provide(redis)
        .module::<SeededProducerModule>()
        .build_headless()
        .await
        .expect("a producer-only app boots");
    let producer = RedisQueueProducer::clone(
        &app.container()
            .get::<RedisQueueProducer>()
            .expect("the producer binding"),
    );
    Box::leak(Box::new(app));
    producer
}

// --- cancellation --------------------------------------------------------------------

/// A job no worker has taken is cancelled — `true`, once — and its records go
/// with it; a job the backend never knew, and a key no job holds, answer
/// `false`. A cancel names a job by its receipt, which a caller may keep as data
/// and rebuild, and never by apalis's own id.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_answers_true_once_for_a_waiting_job_and_false_for_one_never_known() {
    let producer = crate::producer().await;
    let queue = format!("nestrs-e2e-cancel-waiting-{}", crate::this_run());
    let receipt = producer
        .push_json(
            &queue,
            json!({ "take": 1 }),
            PushOptions::default().with_unique("song-1"),
        )
        .await
        .expect("a push nothing drains");
    let kept: PushReceipt = serde_json::from_value(serde_json::to_value(&receipt).expect("data"))
        .expect("a receipt read back");

    assert!(
        producer.cancel(&kept).await.expect("a cancel"),
        "the job waited, so it never starts"
    );
    assert!(
        !producer.cancel(&kept).await.expect("a second cancel"),
        "a cancelled job is not cancelled twice",
    );
    let open = crate::key_of(&queue, "open", &receipt.id().to_string());
    let tombstone = crate::key_of(&queue, "cancelled", &receipt.id().to_string());
    assert_eq!(crate::read(&open).await, None, "its open record is closed");
    assert_eq!(
        crate::read(&crate::key_of(&queue, "unique", "song-1")).await,
        None,
        "and its unique key free",
    );
    assert!(
        crate::pttl(&tombstone).await > 0,
        "while its tombstone waits for the delivery that would have run it",
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
    assert!(
        took < budget * 3 / 2,
        "one budget, not the check's and the filing's in a row: took {took:?}"
    );
}

/// A unique claim that ran on Redis and whose answer was lost holds its key for
/// the claim's hold, not a week, and says so naming the job and the key. The job
/// was never filed; the claim held the key a week with nothing logged, and every
/// retry under it was refused naming a job that exists nowhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_unique_claim_whose_answer_is_lost_lapses_within_its_hold_and_says_so() {
    let logs = nest_rs_testing::LogCapture::install();
    let proxy = crate::MutingProxy::start().await;
    let conn = RedisConnection::connect(&RedisConfig {
        url: proxy.url(),
        connect_timeout: Duration::from_secs(1),
        ..RedisConfig::default()
    })
    .await
    .expect("connect through the proxy");
    let producer = RedisQueueProducer::new(conn);
    let queue = format!("nestrs-e2e-unique-lost-{}", crate::this_run());
    let under = |key: &str| PushOptions::default().with_unique(key);

    // Answered: the scripts are in Redis's cache, and the claim is extended to
    // the job's keeping once its filing is confirmed.
    producer
        .push_json(&queue, json!({ "n": 1 }), under("answered"))
        .await
        .expect("a push Redis answers");
    let answered = crate::pttl(&crate::key_of(&queue, "unique", "answered")).await;
    assert!(
        answered > 6 * 24 * 60 * 60 * 1000,
        "a confirmed job keeps its key for its keeping, not {answered} ms"
    );

    proxy.mute();
    let refused = producer
        .push_json(&queue, json!({ "n": 2 }), under("lost"))
        .await
        .expect_err("the claim's answer never arrives");
    assert!(matches!(refused, QueueError::Backend(_)), "{refused:?}");

    let claim = crate::key_of(&queue, "unique", "lost");
    let holder = crate::read(&claim)
        .await
        .expect("the claim ran on Redis all the same");
    let held = crate::pttl(&claim).await;
    assert!(
        held > 0 && held <= 40_000,
        "held for the claim's hold, twice the port's net, not {held} ms"
    );
    let said = logs.expect_one(
        nest_rs_queue::TARGET,
        "unique key claimed without its job confirmed queued; the claim lapses within its hold \
         unless a worker admits the job",
    );
    assert_eq!(said.level, "warn");
    assert_eq!(said.field("step").as_deref(), Some("claim unanswered"));
    assert_eq!(said.field("job_id"), Some(holder));
    assert_eq!(said.field("unique_key").as_deref(), Some("lost"));

    crate::forget(&queue).await;
}
