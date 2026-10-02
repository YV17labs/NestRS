//! Where a queue lives, against a live Redis.
//!
//! - **The 6.x layout is never ignored.** Jobs a 6.x release left at the root of
//!   the keyspace, under the queue's bare name — waiting, held back for later, or
//!   in flight — are where a 7.0 worker never reads, so a worker serving that
//!   queue refuses to start and names the way out, and a producer pushing to it
//!   says once that they wait there. A key of another type at one of those names
//!   is an application's, named and left alone, and hides no 6.x job beside it.
//! - **The way out the documentation prescribes is run, not described.** The move
//!   the queue pages publish — every structure renamed under the namespace, the
//!   in-flight set registered as a consumer — is played on a layout written the
//!   way 6.x wrote it, and a 7.0 worker then runs every job it held exactly once:
//!   the one waiting, the one held back, and the one a 6.x replica died running.
//! - **Everything a queue holds is under its namespace, and the queue page's ACL
//!   runs it.** A Redis user created exactly as the page prescribes — reaching
//!   `nestrs:queue:*` and nothing else, allowed the commands the page lists and
//!   nothing else — runs a queue end to end: a push, a delayed push, a
//!   completion, a retry and a dead letter, with every apalis script the worker
//!   runs them through. The database holds no other key afterwards, and Redis's
//!   own ACL log holds no denial but the 6.x check's.
//!
//! Every name at the root is built from the queue's, never written whole: this
//! suite plays the layout the framework left behind, and a literal of it would
//! read, to the keys join, as the framework still writing it.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apalis::prelude::{Monitor, Storage, WorkerBuilder, WorkerFactoryFn};
use apalis_redis::{Config, RedisStorage};
use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobId, JobProducerExt, PushOptions, processor, queue};
use nest_rs_redis::{
    RedisConnection, RedisModule, RedisQueueModule, RedisWorker, RedisWorkerModule,
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

/// A job id a 6.x producer held back for later, under the queue's bare name.
async fn leave_a_6x_job_scheduled(admin: &mut RedisConnection, queue: &str) -> String {
    let scheduled = format!("{queue}:scheduled");
    let _: i64 = redis::cmd("ZADD")
        .arg(&scheduled)
        .arg(4_102_444_800_i64)
        .arg("01KYQ7RH444ZAHA2JP8JBJVRA2")
        .query_async(admin)
        .await
        .expect("ZADD");
    scheduled
}

/// Remove what an earlier run of a test left for `queue` at the root, so a
/// count starts from the jobs this run leaves.
async fn clear_the_root(admin: &mut RedisConnection, queue: &str) {
    let stale = left_at_the_root(admin, queue).await;
    if !stale.is_empty() {
        let _: i64 = redis::cmd("DEL")
            .arg(&stale)
            .query_async(admin)
            .await
            .expect("DEL");
    }
}

/// Whether the worker of `M`'s app refuses to start, and with what — `None`
/// when it starts, in which case it is stopped again.
async fn worker_refusal<M: nest_rs_core::Module + 'static>() -> Option<String> {
    let app = TestApp::builder()
        .module::<M>()
        .build_headless()
        .await
        .expect("the app boots; the worker is what refuses");
    app.init().await.expect("init phases");
    let started = app.spawn_transport(RedisWorker::default()).await;
    Box::leak(Box::new(app));
    match started {
        Ok(worker) => {
            worker.shutdown().await.expect("clean shutdown");
            None
        }
        Err(refused) => Some(refused.to_string()),
    }
}

/// A worker serving a queue whose jobs wait under the 6.x layout does not start:
/// the boot names the queue, each key holding them with how many, and both ways
/// out — the jobs held back for later as much as the ones waiting. Once they are
/// gone it starts. A producer pushing to the queue meanwhile says so once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn jobs_left_under_the_6x_layout_refuse_the_worker_and_warn_the_producer_once() {
    let logs = LogCapture::install_global();
    let mut admin = crate::connect().await;
    clear_the_root(&mut admin, LEGACY_QUEUE).await;
    let waiting = leave_a_6x_job(&mut admin, LEGACY_QUEUE).await;
    let scheduled = leave_a_6x_job_scheduled(&mut admin, LEGACY_QUEUE).await;

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
        &format!("1 waiting on {waiting}"),
        &format!("1 held for later on {scheduled}"),
        "6.x key layout",
        "run a 6.x worker until those keys are gone",
        "RENAMENX",
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
    let refused = app
        .spawn_transport(RedisWorker::default())
        .await
        .err()
        .expect("a job held back for later is a 6.x job too")
        .to_string();
    assert!(
        refused.contains(&format!("1 held for later on {scheduled}")),
        "{refused}"
    );

    let _: i64 = redis::cmd("DEL")
        .arg(&scheduled)
        .query_async(&mut admin)
        .await
        .expect("DEL");
    let worker = app
        .spawn_transport(RedisWorker::default())
        .await
        .expect("with the 6.x jobs gone, the worker starts");
    worker.shutdown().await.expect("clean shutdown");
}

// --- an application's key at a 6.x name ------------------------------------------

const COLLIDING_QUEUE: &str = "nestrs-e2e-layout-colliding";

#[queue(name = "nestrs-e2e-layout-colliding", job = LayoutCommand)]
struct CollidingQueue;

#[injectable]
#[derive(Default)]
struct CollidingProcessor;

#[processor]
impl CollidingProcessor {
    #[process(queue = CollidingQueue, retries = 0)]
    async fn run(&self, _job: LayoutCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [CollidingProcessor],
)]
struct CollidingModule;

/// The line naming the keys at a 6.x name that hold something else.
const FOREIGN: &str = "a key at a 6.x queue name holds what 6.x never kept there; left alone, and not counted as \
     jobs";

/// An application's own key that happens to sit at the name 6.x kept a queue's
/// waiting list under — a string, here — holds no job: the check reads each
/// name's type before it counts, so the worker starts, the key is left as it
/// was, and the boot names it at `warn`, since a structure the check cannot
/// read is one it cannot vouch for. Counted by name, it refused the boot and
/// printed a `RENAMENX` that would have put a string where apalis pushes,
/// failing every push and fetch after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_applications_key_at_a_6x_name_is_not_taken_for_jobs() {
    let logs = LogCapture::install_global();
    let mut admin = crate::connect().await;
    let flag = format!("{COLLIDING_QUEUE}:active");
    let _: () = redis::cmd("SET")
        .arg(&flag)
        .arg("an application's own flag")
        .query_async(&mut admin)
        .await
        .expect("SET");

    let started = crate::replica::<CollidingModule>().await;
    started.worker.shutdown().await.expect("clean shutdown");
    let kept: Option<String> = redis::cmd("GET")
        .arg(&flag)
        .query_async(&mut admin)
        .await
        .expect("GET");
    let _: i64 = redis::cmd("DEL")
        .arg(&flag)
        .query_async(&mut admin)
        .await
        .expect("DEL");

    assert_eq!(
        kept.as_deref(),
        Some("an application's own flag"),
        "left as it was"
    );
    let said: Vec<_> = logs
        .find(nest_rs_queue::TARGET, FOREIGN)
        .into_iter()
        .filter(|event| event.field("queue").as_deref() == Some(COLLIDING_QUEUE))
        .collect();
    assert_eq!(said.len(), 1, "the boot says why: {said:#?}");
    assert_eq!(said[0].level, "warn");
    assert_eq!(
        said[0].field("keys"),
        Some(format!("{flag} (string)")),
        "naming the key and what it holds",
    );
}

const BESIDE_QUEUE: &str = "nestrs-e2e-layout-beside";

#[queue(name = "nestrs-e2e-layout-beside", job = LayoutCommand)]
struct BesideQueue;

#[injectable]
#[derive(Default)]
struct BesideProcessor;

#[processor]
impl BesideProcessor {
    #[process(queue = BesideQueue, retries = 0)]
    async fn run(&self, _job: LayoutCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [BesideProcessor],
)]
struct BesideModule;

/// An application's keys at 6.x names beside a real 6.x job — a counter at the
/// name apalis kept failed jobs under, a flag at the consumers name — hide
/// nothing: each name is read on its own, so the job waiting is still found and
/// the worker still refuses to start beside it, while the keys that are not
/// 6.x's are named and left alone. Read through one script of apalis's, a single
/// such key failed the whole count, and the worker started beside the job.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_applications_key_beside_6x_jobs_hides_none_of_them() {
    let logs = LogCapture::install_global();
    let mut admin = crate::connect().await;
    clear_the_root(&mut admin, BESIDE_QUEUE).await;
    let waiting = leave_a_6x_job(&mut admin, BESIDE_QUEUE).await;
    let counter = format!("{BESIDE_QUEUE}:failed");
    let flag = format!("{BESIDE_QUEUE}:consumers");
    for key in [&counter, &flag] {
        let _: () = redis::cmd("SET")
            .arg(key)
            .arg("3")
            .query_async(&mut admin)
            .await
            .expect("SET");
    }

    let refused = worker_refusal::<BesideModule>().await;
    let _: i64 = redis::cmd("DEL")
        .arg(&waiting)
        .arg(&counter)
        .arg(&flag)
        .query_async(&mut admin)
        .await
        .expect("DEL");

    let refused = refused.expect("the worker refuses to start beside the 6.x job");
    assert!(
        refused.contains(&format!("1 waiting on {waiting}")),
        "{refused}"
    );
    let said: Vec<_> = logs
        .find(nest_rs_queue::TARGET, FOREIGN)
        .into_iter()
        .filter(|event| event.field("queue").as_deref() == Some(BESIDE_QUEUE))
        .collect();
    assert_eq!(said.len(), 1, "{said:#?}");
    assert_eq!(
        said[0].field("keys"),
        Some(format!("{flag} (string)")),
        "the consumers name is read and named; the failed one holds no job and is never read",
    );
}

// --- moving a queue out of the 6.x layout ------------------------------------------

const MOVED_QUEUE: &str = "nestrs-e2e-layout-moved";

/// Every structure apalis keeps for a queue besides its in-flight sets, in the
/// order the upgrade page's loop renames them. The page and this list are one
/// procedure: a structure the page left out is one this test would leave at the
/// root, where the check below fails.
const STRUCTURES: [&str; 9] = [
    "active",
    "scheduled",
    "data",
    "data::result",
    "done",
    "dead",
    "failed",
    "signal",
    "consumers",
];

static MOVED: Runs = Runs::new();

/// When each moved job started, on the wall clock its due second is counted on.
static MOVED_AT: Mutex<Vec<(u64, SystemTime)>> = Mutex::new(Vec::new());

#[queue(name = "nestrs-e2e-layout-moved", job = LayoutCommand)]
struct MovedQueue;

#[injectable]
#[derive(Default)]
struct MovedProcessor;

#[processor]
impl MovedProcessor {
    #[process(queue = MovedQueue, retries = 0)]
    async fn run(&self, job: LayoutCommand) -> anyhow::Result<()> {
        MOVED_AT
            .lock()
            .expect("lock")
            .push((job.run, SystemTime::now()));
        MOVED.start(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [MovedProcessor],
)]
struct MovedModule;

/// A job as 6.x sealed it: the wire envelope before it carried a job id or an
/// attempt. The version is the one 6.x wrote, spelled rather than read, so a
/// 7.x that stops reading it fails here and sends the upgrade page back to be
/// rewritten.
fn sealed_by_6x(run: u64) -> serde_json::Value {
    serde_json::json!({ "v": 1, "payload": { "run": run, "fail": false } })
}

/// The storage a 6.x producer and worker opened for `queue`: apalis's defaults
/// under the queue's bare name, fetching one job per poll.
fn storage_of_6x(
    conn: &RedisConnection,
    queue: &str,
) -> RedisStorage<serde_json::Value, RedisConnection> {
    RedisStorage::new_with_config(
        conn.clone(),
        Config::default().set_namespace(queue).set_buffer_size(1),
    )
}

/// A 6.x replica killed mid-job: an apalis worker consuming under the queue's
/// bare name — the id every 6.x replica shared — takes the one job waiting, and
/// dies with it in flight.
async fn leave_a_6x_job_in_flight(admin: &mut RedisConnection, queue: &str) {
    let worker = WorkerBuilder::new(queue)
        .backend(storage_of_6x(admin, queue))
        .build_fn(|_job: serde_json::Value| {
            std::future::pending::<Result<(), Box<dyn std::error::Error + Send + Sync>>>()
        });
    let running = tokio::spawn(Monitor::new().register(worker).run());
    let in_flight = format!("{queue}:inflight:{queue}");
    let mut taken = 0;
    for _ in 0..200 {
        taken = redis::cmd("SCARD")
            .arg(&in_flight)
            .query_async::<i64>(admin)
            .await
            .expect("SCARD");
        if taken == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    running.abort();
    let _ = running.await;
    assert_eq!(taken, 1, "the 6.x replica died holding one job");
}

/// Rename `from` to `to` the way the documented loop does — `RENAMENX`, which
/// refuses to overwrite a key 7.0 already wrote — answering whether `from` was
/// there to move.
async fn rename_if_there(admin: &mut RedisConnection, from: &str, to: &str) -> bool {
    match redis::cmd("RENAMENX")
        .arg(from)
        .arg(to)
        .query_async::<i64>(admin)
        .await
    {
        Ok(1) => true,
        Ok(_) => panic!("`{to}` already exists; the move would have lost `{from}`"),
        Err(error) if error.to_string().contains("no such key") => false,
        Err(error) => panic!("RENAMENX {from}: {error}"),
    }
}

/// The move the upgrade page publishes, as an operator runs it: each structure
/// renamed to the same structure under the namespace, the in-flight set with
/// them, and that set registered as a consumer, so the first sweep of a 7.0
/// worker puts its jobs back on the queue.
async fn move_out_of_the_6x_layout(admin: &mut RedisConnection, queue: &str) {
    let namespace = crate::namespace(queue);
    let in_flight = format!("inflight:{queue}");
    for structure in STRUCTURES.into_iter().chain([in_flight.as_str()]) {
        rename_if_there(
            admin,
            &format!("{queue}:{structure}"),
            &format!("{namespace}:{structure}"),
        )
        .await;
    }
    let _: i64 = redis::cmd("ZADD")
        .arg(format!("{namespace}:consumers"))
        .arg(0)
        .arg(format!("{namespace}:{in_flight}"))
        .query_async(admin)
        .await
        .expect("ZADD");
}

/// Every key left for `queue` at the root of the keyspace.
async fn left_at_the_root(admin: &mut RedisConnection, queue: &str) -> Vec<String> {
    redis::cmd("KEYS")
        .arg(format!("{queue}:*"))
        .query_async(admin)
        .await
        .expect("KEYS")
}

/// A queue a 6.x release left jobs in — one waiting, one held back, one in
/// flight on a replica that died — refuses the 7.0 worker, counting each, and
/// once moved the way the upgrade page says, nothing is left at the root, the
/// worker starts, and each of the three jobs runs exactly once — the held-back
/// one no sooner than its due second.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queue_moved_out_of_the_6x_layout_runs_every_job_it_held_once() {
    let mut admin = crate::connect().await;
    clear_the_root(&mut admin, MOVED_QUEUE).await;
    crate::forget(MOVED_QUEUE).await;

    let run = crate::this_run();
    let (in_flight, waiting, held_back) = (run, run + 1, run + 2);
    let mut storage = storage_of_6x(&admin, MOVED_QUEUE);
    storage
        .push(sealed_by_6x(in_flight))
        .await
        .expect("a 6.x push");
    leave_a_6x_job_in_flight(&mut admin, MOVED_QUEUE).await;
    storage
        .push(sealed_by_6x(waiting))
        .await
        .expect("a 6.x push");
    let due = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs()
        + 2;
    storage
        .schedule(
            sealed_by_6x(held_back),
            i64::try_from(due).expect("a second"),
        )
        .await
        .expect("a 6.x job held back");

    // The boot counts exactly what 6.x left — waiting, held back and in flight.
    let refused = worker_refusal::<MovedModule>()
        .await
        .expect("the worker refuses to start beside the 6.x jobs");
    for held in [
        format!("1 waiting on {MOVED_QUEUE}:active"),
        format!("1 held for later on {MOVED_QUEUE}:scheduled"),
        format!("1 in flight in {MOVED_QUEUE}:inflight:{MOVED_QUEUE}"),
    ] {
        assert!(refused.contains(&held), "{held:?} in {refused}");
    }

    // The page's first step: nothing under the namespace yet, or no move at all.
    let under_the_namespace: Vec<String> = redis::cmd("KEYS")
        .arg(format!("{}:*", crate::namespace(MOVED_QUEUE)))
        .query_async(&mut admin)
        .await
        .expect("KEYS");
    assert!(
        under_the_namespace.is_empty(),
        "the move starts from an empty namespace: {under_the_namespace:?}"
    );
    move_out_of_the_6x_layout(&mut admin, MOVED_QUEUE).await;
    let stale = left_at_the_root(&mut admin, MOVED_QUEUE).await;
    assert!(stale.is_empty(), "nothing is left at the root: {stale:?}");

    let replica = crate::replica::<MovedModule>().await;
    crate::wait_until(Duration::from_secs(20), || {
        [in_flight, waiting, held_back]
            .iter()
            .all(|run| !MOVED.of(*run).is_empty())
    })
    .await;
    // Long enough for a second delivery of any of them to have run.
    tokio::time::sleep(Duration::from_secs(2)).await;
    replica.worker.shutdown().await.expect("clean shutdown");

    for (job, run) in [
        ("in flight", in_flight),
        ("waiting", waiting),
        ("held back", held_back),
    ] {
        assert_eq!(MOVED.of(run).len(), 1, "the job {job} ran exactly once");
    }
    let moved_in_flight = format!("{}:inflight:{MOVED_QUEUE}", crate::namespace(MOVED_QUEUE));
    let still_there: i64 = redis::cmd("EXISTS")
        .arg(&moved_in_flight)
        .query_async(&mut admin)
        .await
        .expect("EXISTS");
    assert_eq!(still_there, 0, "the sweep emptied the moved in-flight set");
    let started = MOVED_AT
        .lock()
        .expect("lock")
        .iter()
        .find(|(run, _)| *run == held_back)
        .map(|(_, at)| *at)
        .expect("the job held back started");
    assert!(
        started
            .duration_since(UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs()
            >= due,
        "the job held back ran no sooner than its due second",
    );

    crate::forget(MOVED_QUEUE).await;
}

// --- a user created as the queue page prescribes --------------------------------------

/// The ACL user this test creates from the page, and removes — named for this
/// run, so no earlier run's denial in `ACL LOG` is read as its own.
const CONFINED_USER: &str = "nestrs-e2e-confined";

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

/// The user the queue page prescribes runs a queue from the push to the dead
/// letter: a completion, a delayed push, a retry filed on the schedule and a
/// spent budget — every apalis script a producer and a worker send, and every
/// one of the framework's. Nothing it needs lies outside the namespace or the
/// commands the page lists — the ACL refuses anything that does, the scripts'
/// own commands included — and the database holds no other key afterwards. The
/// 6.x check the ACL cannot reach is said, not passed over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_user_created_as_the_queue_page_says_runs_a_queue_from_push_to_dead_letter() {
    let logs = LogCapture::install_global();
    let mut admin = RedisConnection::connect(&crate::redis_config_on(DB_CONFINED_TO_THE_PREFIX))
        .await
        .expect("the admin connection");
    let _: () = redis::cmd("FLUSHDB")
        .query_async(&mut admin)
        .await
        .expect("FLUSHDB");
    let user = crate::acl_user(CONFINED_USER);
    let confined =
        crate::documented_user("queue/delivery.mdx", &user, DB_CONFINED_TO_THE_PREFIX).await;
    crate::assert_may_load_a_script(&confined).await;

    let run = crate::this_run();
    let replica = crate::replica_on::<ConfinedModule>(confined).await;
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
    crate::forget_user(&user).await;

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
        .filter(crate::refused_by_acl)
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

    // Redis's own record: the 6.x check's reads, refused on purpose, and nothing
    // else — a refusal some path swallowed would be here even without a line.
    crate::assert_redis_denied_nothing_but(&user, &["type", "llen", "zcard", "zrange", "scard"])
        .await;

    let outside: Vec<String> = every_key(&mut admin)
        .await
        .into_iter()
        .filter(|key| !key.split(':').take(2).eq(["nestrs", "queue"]))
        .collect();
    assert!(
        outside.is_empty(),
        "no key outside the queue's namespace: {outside:?}"
    );
}
