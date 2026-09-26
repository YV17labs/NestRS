//! Where a queue lives, against a live Redis.
//!
//! - **The 6.x layout is never ignored.** Jobs a 6.x release left at the root of
//!   the keyspace, under the queue's bare name, are where a 7.0 worker never
//!   reads, so a worker serving that queue refuses to start and names the way
//!   out, and a producer pushing to it says once that they wait there.
//! - **Everything a queue holds is under the framework's prefix.** A Redis user
//!   whose ACL reaches `nestrs:*` and nothing else runs a queue end to end — a
//!   push, a delayed push, a completion, a retry and a dead letter — and the
//!   database holds no other key afterwards. The ACL is the proof: a single
//!   command outside the prefix is refused, and the run would not complete.
//!
//! Every name at the root is built from the queue's, never written whole: this
//! suite plays the layout the framework left behind, and a literal of it would
//! read, to the keys join, as the framework still writing it.

use std::sync::Mutex;
use std::time::Duration;

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobId, JobProducerExt, PushOptions, processor, queue};
use nest_rs_redis::{
    RedisConfig, RedisConnection, RedisModule, RedisQueueModule, RedisWorker, RedisWorkerModule,
};
use nest_rs_testing::{LogCapture, TestApp};
use serde::{Deserialize, Serialize};

use crate::{DB_CONFINED_TO_THE_PREFIX, Runs};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LayoutCommand {
    run: u64,
    /// Whether every attempt fails, retryably.
    fail: bool,
}

// --- the 6.x layout ------------------------------------------------------------------

const LEGACY_QUEUE: &str = "nestrs-e2e-layout-legacy";

#[queue(name = "nestrs-e2e-layout-legacy", job = LayoutCommand)]
struct LegacyQueue;

#[injectable]
#[derive(Default)]
struct LegacyProcessor;

#[processor]
impl LegacyProcessor {
    #[process(queue = LegacyQueue, retries = 0)]
    async fn run(&self, _job: LayoutCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [LegacyProcessor],
)]
struct LegacyModule;

/// A job id a 6.x worker left waiting, under the queue's bare name.
async fn leave_a_6x_job(admin: &mut RedisConnection, queue: &str) -> String {
    let waiting = format!("{queue}:active");
    let _: i64 = redis::cmd("RPUSH")
        .arg(&waiting)
        .arg("01KYQ7RH444ZAHA2JP8JBJVRA1")
        .query_async(admin)
        .await
        .expect("RPUSH");
    waiting
}

/// A worker serving a queue whose jobs wait under the 6.x layout does not start:
/// the boot names the queue, the key holding them and both ways out. Once they
/// are gone it starts. A producer pushing to the queue meanwhile says so once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jobs_left_under_the_6x_layout_refuse_the_worker_and_warn_the_producer_once() {
    let logs = LogCapture::install_global();
    let mut admin = crate::connect().await;
    let waiting = leave_a_6x_job(&mut admin, LEGACY_QUEUE).await;

    let app = TestApp::builder()
        .module::<LegacyModule>()
        .build_headless()
        .await
        .expect("the app boots; the worker is what refuses");
    app.init().await.expect("init phases");
    let refused = app
        .spawn_transport(RedisWorker::default())
        .await
        .err()
        .expect("the worker refuses to start beside 6.x jobs")
        .to_string();
    for expected in [
        LEGACY_QUEUE,
        waiting.as_str(),
        "6.x key layout",
        "run a 6.x worker until those keys are gone",
        "RENAME",
        &crate::namespace(LEGACY_QUEUE),
    ] {
        assert!(refused.contains(expected), "{expected:?} in {refused}");
    }

    let producer = app
        .container()
        .get::<nest_rs_redis::RedisQueueProducer>()
        .expect("the producer binding");
    for fail in [false, true] {
        producer
            .push(LegacyQueue, LayoutCommand { run: 0, fail }, None)
            .await
            .expect("a push lands where 7.0 reads, 6.x jobs or not");
    }
    let warned: Vec<_> = logs
        .find(
            nest_rs_queue::TARGET,
            "jobs wait under the 6.x key layout; a 7.0 worker does not run them — drain them \
             with a 6.x worker, or move them under the queue's namespace",
        )
        .into_iter()
        .filter(|event| event.field("queue").as_deref() == Some(LEGACY_QUEUE))
        .collect();
    assert_eq!(warned.len(), 1, "said once per queue: {warned:#?}");
    assert_eq!(warned[0].level, "warn");

    let _: i64 = redis::cmd("DEL")
        .arg(&waiting)
        .query_async(&mut admin)
        .await
        .expect("DEL");
    let worker = app
        .spawn_transport(RedisWorker::default())
        .await
        .expect("with the 6.x jobs gone, the worker starts");
    worker.shutdown().await.expect("clean shutdown");
}

// --- a user confined to the framework's prefix ---------------------------------------

/// The ACL user this test creates, and removes.
const CONFINED_USER: &str = "nestrs-e2e-confined";

/// The confined user's password — a test fixture, never a secret.
const CONFINED_PASSWORD: &str = "confined-to-the-prefix";

/// The URL the confined user reaches its own database with.
fn confined_config() -> RedisConfig {
    let url = crate::redis_url_on(DB_CONFINED_TO_THE_PREFIX).replacen(
        "://",
        &format!("://{CONFINED_USER}:{CONFINED_PASSWORD}@"),
        1,
    );
    RedisConfig {
        url,
        ..Default::default()
    }
}

static CONFINED: Runs = Runs::new();

/// The job ids each outcome was reached for.
static COMPLETED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

#[queue(name = "nestrs-e2e-layout-confined", job = LayoutCommand)]
struct ConfinedQueue;

#[injectable]
#[derive(Default)]
struct ConfinedProcessor;

#[processor]
impl ConfinedProcessor {
    #[process(queue = ConfinedQueue, retries = 1)]
    async fn run(&self, job: LayoutCommand) -> anyhow::Result<()> {
        CONFINED.start(job.run);
        if job.fail {
            anyhow::bail!("the upstream is gone");
        }
        COMPLETED.lock().expect("lock").push(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(None), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [ConfinedProcessor],
)]
struct ConfinedModule;

/// Whether the job `id` ended on its second attempt's failure — with
/// `retries = 1`, the attempt that spends the budget — and the port said it
/// dead-lettered a job for a spent budget.
fn dead_letters(logs: &LogCapture, id: &JobId) -> bool {
    let last_failed = logs
        .find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_queue::unit::JOB,
        )
        .iter()
        .any(|line| {
            crate::names(line, id)
                && line.field("attempt").as_deref() == Some("2")
                && line.field("outcome").as_deref() == Some(nest_rs_core::operation_log::ERROR)
        });
    last_failed
        && !logs
            .find(
                nest_rs_queue::TARGET,
                "job dead-lettered: retry budget spent",
            )
            .is_empty()
}

/// Every key in the confined database.
async fn every_key(admin: &mut RedisConnection) -> Vec<String> {
    let mut cursor = 0u64;
    let mut keys = Vec::new();
    loop {
        let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("COUNT")
            .arg(1000)
            .query_async(admin)
            .await
            .expect("SCAN");
        keys.extend(batch);
        if next == 0 {
            return keys;
        }
        cursor = next;
    }
}

/// A user whose ACL reaches `nestrs:*` and no other key runs a queue from the
/// push to the dead letter: a completion, a delayed push, a retry filed on the
/// schedule and a spent budget. Nothing it needs lies outside the prefix — the
/// ACL refuses anything that does — and the database holds no other key
/// afterwards. The 6.x check the ACL cannot reach is said, not passed over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_user_confined_to_the_prefix_runs_a_queue_from_push_to_dead_letter() {
    let logs = LogCapture::install_global();
    let mut admin = RedisConnection::connect(&crate::redis_config_on(DB_CONFINED_TO_THE_PREFIX))
        .await
        .expect("the admin connection");
    let _: () = redis::cmd("FLUSHDB")
        .query_async(&mut admin)
        .await
        .expect("FLUSHDB");
    let _: () = redis::cmd("ACL")
        .arg("SETUSER")
        .arg(CONFINED_USER)
        .arg("reset")
        .arg("on")
        .arg(format!(">{CONFINED_PASSWORD}"))
        .arg("~nestrs:*")
        .arg("+@all")
        .arg("-@dangerous")
        .query_async(&mut admin)
        .await
        .expect("ACL SETUSER");

    let run = crate::this_run();
    let replica = crate::replica_on::<ConfinedModule>(confined_config()).await;
    let completed = replica
        .producer
        .push(ConfinedQueue, LayoutCommand { run, fail: false }, None)
        .await
        .expect("an immediate push");
    let delayed = replica
        .producer
        .push(
            ConfinedQueue,
            LayoutCommand {
                run: run + 1,
                fail: false,
            },
            PushOptions::default().with_delay(Duration::from_secs(1)),
        )
        .await
        .expect("a delayed push");
    let doomed = replica
        .producer
        .push(
            ConfinedQueue,
            LayoutCommand {
                run: run + 2,
                fail: true,
            },
            None,
        )
        .await
        .expect("a push that fails every attempt");

    crate::wait_until(Duration::from_secs(20), || {
        let done = COMPLETED.lock().expect("lock").clone();
        done.contains(&run) && done.contains(&(run + 1)) && dead_letters(&logs, doomed.id())
    })
    .await;
    replica.worker.shutdown().await.expect("clean shutdown");
    let _: () = redis::cmd("ACL")
        .arg("DELUSER")
        .arg(CONFINED_USER)
        .query_async(&mut admin)
        .await
        .expect("ACL DELUSER");

    let done = COMPLETED.lock().expect("lock").clone();
    assert!(
        done.contains(&run),
        "the immediate job {} completed",
        completed.id()
    );
    assert!(
        done.contains(&(run + 1)),
        "the delayed job {} completed",
        delayed.id()
    );
    assert_eq!(
        CONFINED.of(run + 2).len(),
        2,
        "the doomed job ran its two attempts"
    );
    assert!(dead_letters(&logs, doomed.id()), "and was dead-lettered");
    // The one thing the ACL refused is the 6.x check, at the root — said by the
    // worker, at `warn`, and by the producer as detail.
    let worker_unchecked = "6.x key layout not checked: the connection's ACL does not reach it; \
                            drain any 6.x jobs on this database before relying on this worker";
    let producer_unchecked = "6.x key layout not checked: the connection's ACL does not reach it";
    let refused: Vec<_> = logs
        .events()
        .into_iter()
        .filter(|event| {
            event
                .field("error")
                .is_some_and(|error| error.contains("NOPERM"))
        })
        .collect();
    assert!(
        refused
            .iter()
            .all(|event| event.message == worker_unchecked || event.message == producer_unchecked),
        "nothing but the 6.x check met the ACL: {refused:#?}",
    );
    let unchecked = logs.find(nest_rs_queue::TARGET, worker_unchecked);
    assert!(
        unchecked.len() == 1 && unchecked[0].level == "warn",
        "the worker said, once and at `warn`, that it could not check: {unchecked:#?}",
    );

    let outside: Vec<String> = every_key(&mut admin)
        .await
        .into_iter()
        .filter(|key| !key.starts_with("nestrs:"))
        .collect();
    assert!(outside.is_empty(), "no key outside the prefix: {outside:?}");
}
