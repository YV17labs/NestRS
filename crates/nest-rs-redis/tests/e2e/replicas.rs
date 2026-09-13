//! Two replicas of the same worker app on one queue — the deployment shape the
//! queue's concurrency contract *depends* on. Throughput comes from replicas
//! rather than from an in-process ceiling (see `concurrency.rs`), so how two of
//! them share a queue is part of that contract, not an edge case.
//!
//! Two separate questions.
//!
//! # 1. Does the fetch hand one job to two replicas? No.
//!
//! A replica claims a job with one `LMOVE` from the queue's list into its own
//! processing list. Redis runs a command atomically, so a second poller cannot
//! observe an id the first has claimed. That is the exclusive-delivery
//! guarantee, and [`the_fetch_never_hands_one_job_to_two_replicas`] measures it.
//!
//! # 2. Does starting a replica disturb jobs already in flight? No.
//!
//! A job goes back on its queue only once the process holding it has stopped
//! heartbeating; nothing sweeps at startup.
//! [`a_replica_starting_mid_flight_leaves_the_in_flight_job_alone`] measures it.
//!
//! Both run in one test process, and that bounds what the two tests can prove.
//! oxana identifies a process by hostname and pid, so the two replicas share an
//! identity and one processing list. The first question is unaffected — the
//! claim is atomic whichever list it lands in. The second is proved only
//! against a sweep at startup that would take *every* in-flight job: a sweep
//! that spared its own identity and took its peers' would find no peer here and
//! pass this test. Nor can a replica be dead while the other heartbeats for
//! both, so the other half of at-least-once — a replica that dies mid-job has
//! that job run again — needs two processes, and `consumer.rs` states it rather
//! than this suite asserting it.

use std::sync::Mutex;
use std::time::Duration;

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, processor, queue};
use nest_rs_redis::{
    RedisConnection, RedisModule, RedisQueueModule, RedisQueueProducer, RedisWorker,
    RedisWorkerModule,
};
use nest_rs_testing::TestApp;
use serde::{Deserialize, Serialize};

/// Long enough that a second replica can start while a job is still running.
const HOLD: Duration = Duration::from_secs(3);

/// One fixture per test, spelled out twice rather than through a macro: nextest
/// runs tests in parallel, and both a shared `#[queue]` and a shared static would
/// let one test's jobs land in the other's assertion. Separate queue names also
/// keep each test's jobs on a list of their own.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlowCommand {
    seq: usize,
}

// --- fixture 1: the exclusive-delivery guarantee ----------------------------

static FETCH_RUNS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

#[queue(name = "nestrs-e2e-replicas-fetch", job = SlowCommand)]
struct FetchQueue;

#[injectable]
#[derive(Default)]
struct FetchProcessor;

#[processor]
impl FetchProcessor {
    #[process(queue = FetchQueue, retries = 0)]
    async fn slow(&self, job: SlowCommand) -> anyhow::Result<()> {
        FETCH_RUNS.lock().expect("lock").push(job.seq);
        tokio::time::sleep(HOLD).await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [FetchProcessor],
)]
struct FetchModule;

// --- fixture 2: a replica starting mid-flight --------------------------------

static SCALE_UP_RUNS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

#[queue(name = "nestrs-e2e-replicas-scaleup", job = SlowCommand)]
struct ScaleUpQueue;

#[injectable]
#[derive(Default)]
struct ScaleUpProcessor;

#[processor]
impl ScaleUpProcessor {
    #[process(queue = ScaleUpQueue, retries = 0)]
    async fn slow(&self, job: SlowCommand) -> anyhow::Result<()> {
        SCALE_UP_RUNS.lock().expect("lock").push(job.seq);
        tokio::time::sleep(HOLD).await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [ScaleUpProcessor],
)]
struct ScaleUpModule;

/// Boot one "replica": its own app, its own pool, its own `RedisWorker`
/// transport, against the same Redis and the same queue — and, unlike two
/// containers, the same process identity (see the module docs).
async fn spawn_replica<M: nest_rs_core::Module + 'static>() -> nest_rs_testing::TransportHandle {
    let app = TestApp::builder()
        .module::<M>()
        .build_headless()
        .await
        .expect("a worker replica boots against the dev container Redis");
    app.init().await.expect("init phases");
    let handle = app
        .spawn_transport(RedisWorker::default())
        .await
        .expect("the queue worker transport starts");
    // The transport borrows the container the app owns; leak it so the replica
    // outlives this scope, the way a container's process would.
    Box::leak(Box::new(app));
    handle
}

/// The guarantee that makes replica-based throughput sound: with both replicas
/// already up, a batch is split between them and **no job runs twice**. This is
/// the atomic claim, measured rather than read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_fetch_never_hands_one_job_to_two_replicas() {
    const JOBS: usize = 4;

    // Both up before any job exists, so the split is the fetch's alone.
    let first = spawn_replica::<FetchModule>().await;
    let second = spawn_replica::<FetchModule>().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = RedisQueueProducer::new(
        RedisConnection::connect(&crate::redis_config().url)
            .await
            .expect("connect"),
    );
    for seq in 0..JOBS {
        conn.push_to::<FetchQueue>(SlowCommand { seq })
            .await
            .expect("enqueue");
    }

    // Serialized per replica, two replicas ⇒ ceil(JOBS / 2) waves, plus slack.
    tokio::time::sleep(HOLD * (JOBS as u32).div_ceil(2) + Duration::from_secs(3)).await;
    first.shutdown().await.expect("clean shutdown");
    second.shutdown().await.expect("clean shutdown");

    let mut seen = FETCH_RUNS.lock().expect("lock").clone();
    seen.sort_unstable();
    assert_eq!(
        seen,
        (0..JOBS).collect::<Vec<_>>(),
        "every job ran exactly once across the two replicas",
    );
}

/// Starting a replica while a peer holds a job leaves the job with that peer,
/// so it runs once. A scale-up is the moment a deployment adds replicas under
/// load, which is exactly when re-running in-flight work would hurt most.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replica_starting_mid_flight_leaves_the_in_flight_job_alone() {
    let first = spawn_replica::<ScaleUpModule>().await;

    let conn = RedisQueueProducer::new(
        RedisConnection::connect(&crate::redis_config().url)
            .await
            .expect("connect"),
    );
    conn.push_to::<ScaleUpQueue>(SlowCommand { seq: 0 })
        .await
        .expect("enqueue");

    // Let the first replica claim it and reach the handler body.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(
        SCALE_UP_RUNS.lock().expect("lock").len(),
        1,
        "the first replica must be holding the job before the second starts",
    );

    // Scale up mid-flight — the event this test is about.
    let second = spawn_replica::<ScaleUpModule>().await;
    tokio::time::sleep(HOLD + Duration::from_secs(3)).await;

    first.shutdown().await.expect("clean shutdown");
    second.shutdown().await.expect("clean shutdown");

    assert_eq!(
        SCALE_UP_RUNS.lock().expect("lock").clone(),
        vec![0],
        "the in-flight job runs once: a starting replica leaves a live peer's work alone",
    );
}
