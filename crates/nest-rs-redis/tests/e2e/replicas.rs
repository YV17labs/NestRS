//! Two replicas of the same worker app on one queue — the deployment shape the
//! queue's concurrency contract *depends* on. Throughput comes from replicas
//! rather than from an in-process ceiling (see `concurrency.rs`), so how two of
//! them share a queue is part of that contract, not an edge case.
//!
//! Two separate questions, and they have different answers.
//!
//! # 1. Does the fetch hand one job to two replicas? No.
//!
//! `get_jobs.lua` is a single Redis EVAL that `lrange`s the ids, `sadd`s them to
//! the consumer's inflight set and `ltrim`s them off the active list. Redis runs
//! a script atomically, so a second poller cannot observe an id the first has
//! claimed. That is the exclusive-delivery guarantee, and
//! [`the_fetch_never_hands_one_job_to_two_replicas`] measures it.
//!
//! # 2. Does starting a replica disturb jobs already in flight? Yes — a defect.
//!
//! On startup, `RedisStorage::poll` calls
//! `reenqueue_orphaned(limit, Utc::now())`. Its comment says "reenqueue any jobs
//! that **belonged to this worker** in case of a death", but the cutoff is *now*,
//! and `reenqueue_orphaned_jobs.lua` selects `zrangebyscore(consumers, 0, now)`
//! — every registered consumer, including peers that are alive and mid-job. It
//! `spop`s their inflight sets back onto the active list, so a scale-up re-runs
//! whatever was in flight.
//!
//! Our own choice sharpens it: `WorkerBuilder::new(method.queue)` makes the
//! `WorkerId` the queue name verbatim, so every replica registers the *same*
//! consumer identity and shares one inflight set
//! (`{queue}:inflight:{queue}`) — the starting replica pops exactly the peer's
//! in-flight jobs. A unique id per process would not fix the steal (the sweep
//! matches peers either way) but would fix a second, unmeasured consequence:
//! with a shared identity, a crashed replica's in-flight jobs are never
//! reclaimed while any peer keeps the shared heartbeat fresh.
//!
//! [`a_replica_starting_mid_flight_re_runs_the_in_flight_job`] pins the measured
//! behaviour rather than the behaviour we want, so it is green today and turns
//! red the moment either side is fixed — which is the point: the change should be
//! deliberate, and the fix is an upstream decision (apalis-redis 0.7.4).

use std::sync::Mutex;
use std::time::Duration;

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, processor, queue};
use nest_rs_redis::{
    RedisQueueConfig, RedisQueueConnection, RedisQueueModule, RedisWorker, RedisWorkerModule,
};
use nest_rs_testing::TestApp;
use serde::{Deserialize, Serialize};

/// Long enough that a second replica can start while a job is still running.
const HOLD: Duration = Duration::from_secs(3);

/// One fixture per test, spelled out twice rather than through a macro: nextest
/// runs tests in parallel, and both a shared `#[queue]` and a shared static would
/// let one test's jobs land in the other's assertion. Separate queue names also
/// give each test its own Redis key space (`{queue}:*`), so the startup sweep one
/// test triggers cannot reach the other's consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlowCommand {
    seq: usize,
}

fn queue_config() -> RedisQueueConfig {
    RedisQueueConfig {
        url: std::env::var(nest_rs_config::var_name("queue", "URL"))
            .unwrap_or_else(|_| "redis://redis:6379".to_string()),
        ..Default::default()
    }
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
    imports = [RedisQueueModule::for_root(queue_config()), RedisWorkerModule],
    providers = [FetchProcessor],
)]
struct FetchModule;

// --- fixture 2: the scale-up defect ----------------------------------------

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
    imports = [RedisQueueModule::for_root(queue_config()), RedisWorkerModule],
    providers = [ScaleUpProcessor],
)]
struct ScaleUpModule;

/// Boot one "replica": its own app, its own `RedisWorker` transport, against the
/// same Redis and the same queue. Two of these in one process are
/// indistinguishable from two containers as far as the backend is concerned —
/// the consumer identity apalis registers comes from the worker name, which is
/// the queue name, so it is byte-identical either way.
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
/// the atomic claim in `get_jobs.lua`, measured rather than read.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_fetch_never_hands_one_job_to_two_replicas() {
    const JOBS: usize = 4;

    // Both up before any job exists, so no startup sweep can find work in
    // flight — this isolates the fetch from the scale-up defect below.
    let first = spawn_replica::<FetchModule>().await;
    let second = spawn_replica::<FetchModule>().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = RedisQueueConnection::connect(&queue_config().url)
        .await
        .expect("connect");
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

/// **Pins a defect, not a desired behaviour.** Starting a replica while a peer
/// holds a job puts that job back on the queue, and it runs a second time —
/// same `job_id`, both attempts reported as `attempt=1`. Measured against live
/// Redis:
///
/// ```text
/// INFO process job{queue="nestrs-e2e-replicas" job_id=01KYT4X0FN… attempt=1}: job ok elapsed_ms=3002
/// INFO process job{queue="nestrs-e2e-replicas" job_id=01KYT4X0FN… attempt=1}: job ok elapsed_ms=3001
/// ```
///
/// The cause is upstream (see the module docs: a `Utc::now()` cutoff in
/// `RedisStorage::poll`'s startup sweep), so the fix is a dependency decision.
/// Until then this is the honest contract — **a `#[process]` handler must be
/// idempotent**, and a deployment that scales the worker up mid-flight will
/// re-run in-flight jobs, not merely retry failed ones.
///
/// Asserted as-is so that fixing it fails this test loudly instead of silently
/// changing what the queue promises.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replica_starting_mid_flight_re_runs_the_in_flight_job() {
    let first = spawn_replica::<ScaleUpModule>().await;

    let conn = RedisQueueConnection::connect(&queue_config().url)
        .await
        .expect("connect");
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
        vec![0, 0],
        "measured: the in-flight job is requeued by the starting replica and runs \
         twice. If this now fails with `[0]`, the defect is fixed — update the \
         module docs, `/queue/`'s idempotency note, and delete this test in favour \
         of `the_fetch_never_hands_one_job_to_two_replicas`",
    );
}
