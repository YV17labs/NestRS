//! The delivery guard, against live workers: a job apalis delivers twice runs
//! once.
//!
//! apalis-redis delivers **at least once**, and each way it delivers twice is a
//! test here, because each is a path the guard has to meet from a different
//! side:
//!
//! - **a replica starting** puts every peer's in-flight jobs back on the queue —
//!   its startup sweep uses a cutoff of *now*, which matches the living too;
//! - **a sweep that takes a live replica for dead** — its heartbeat fell behind
//!   the threshold a peer judges it by — does the same to that replica alone;
//! - **a replica dying** mid-attempt leaves its job in flight, to be swept and
//!   run again by another, which must not have to wait for anything but the
//!   lease the dead one held;
//! - **the same job filed twice** reaches two deliveries at once.
//!
//! In every case the job runs once — to completion, once overall — the second
//! delivery is handed back while the first holds the lease, and it is
//! acknowledged without running once the first has settled it.
//!
//! Replicas here run [`brisk`](crate::brisk) settings: a two-second lease, so a
//! lease a dead replica held is free again within the test.

use std::time::Duration;

use apalis::prelude::Storage;
use apalis_redis::{Config, RedisStorage};
use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobId, JobProducerExt, processor, queue};
use nest_rs_redis::{
    RedisConnection, RedisModule, RedisQueueModule, RedisWorkerConfig, RedisWorkerModule,
};
use nest_rs_testing::LogCapture;
use serde::{Deserialize, Serialize};

use crate::Runs;

/// The line a second delivery files while the first still runs the job.
const HANDED_BACK: &str = "job delivered while another delivery runs it; handing it back";

/// The line a second delivery files once the first settled the job.
const SETTLED: &str = "job delivered again after it settled; acknowledged without running";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GuardedCommand {
    run: u64,
    /// How long each attempt holds on — the first one only, when `once`.
    hold_ms: u64,
    /// Whether only the first attempt holds on, as a replica dying mid-job
    /// needs: the one that runs it after holds nothing up.
    once: bool,
}

impl GuardedCommand {
    fn holding(run: u64, hold_ms: u64) -> Self {
        Self {
            run,
            hold_ms,
            once: false,
        }
    }
}

/// Run `job`: count its start, hold on, count its end.
async fn run_guarded(runs: &Runs, job: GuardedCommand) {
    let attempt = runs.start(job.run);
    if !job.once || attempt == 1 {
        tokio::time::sleep(Duration::from_millis(job.hold_ms)).await;
    }
    runs.finish(job.run);
}

/// Whether `logs` holds `message` naming the job `id`.
fn said(logs: &LogCapture, message: &str, id: &JobId) -> bool {
    logs.find(nest_rs_queue::TARGET, message)
        .iter()
        .any(|event| crate::names(event, id))
}

/// An apalis handle on `queue`'s namespace — what a peer, or a second producer,
/// reaches the queue through.
async fn apalis_on(queue: &str) -> RedisStorage<serde_json::Value, RedisConnection> {
    RedisStorage::new_with_config(
        crate::connect().await,
        Config::default().set_namespace(&crate::namespace(queue)),
    )
}

// --- a replica starting mid-flight ----------------------------------------------

static SCALED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-replicas-scaleup", job = GuardedCommand)]
struct ScaleUpQueue;

#[injectable]
#[derive(Default)]
struct ScaleUpProcessor;

#[processor]
impl ScaleUpProcessor {
    #[process(queue = ScaleUpQueue, retries = 0)]
    async fn slow(&self, job: GuardedCommand) -> anyhow::Result<()> {
        run_guarded(&SCALED, job).await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [ScaleUpProcessor],
)]
struct ScaleUpModule;

/// apalis's startup sweep hands the job the first replica is running to the
/// queue again, and the replica starting fetches it: the lease sends it back
/// until the first has settled it, and then it is acknowledged without running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replica_starting_mid_flight_never_runs_the_in_flight_job_twice() {
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let first = crate::replica::<ScaleUpModule>().await;
    let receipt = first
        .producer
        .push(ScaleUpQueue, GuardedCommand::holding(run, 3000), None)
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || !SCALED.of(run).is_empty()).await;

    // Scale up mid-flight — the event this test is about.
    let second = crate::replica::<ScaleUpModule>().await;
    crate::wait_until(Duration::from_secs(15), || {
        said(&logs, SETTLED, receipt.id())
    })
    .await;
    first.worker.shutdown().await.expect("clean shutdown");
    second.worker.shutdown().await.expect("clean shutdown");

    assert!(
        said(&logs, HANDED_BACK, receipt.id()),
        "the swept copy was handed back while the job ran",
    );
    assert!(
        said(&logs, SETTLED, receipt.id()),
        "and acknowledged without running once the job settled",
    );
    assert_eq!(SCALED.of(run).len(), 1, "the job ran once");
    assert_eq!(SCALED.finished(run), 1, "to its end");
}

// --- a replica taken for dead while it runs a job --------------------------------

static STALLED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-lease-stall", job = GuardedCommand)]
struct StallQueue;

#[injectable]
#[derive(Default)]
struct StallProcessor;

#[processor]
impl StallProcessor {
    // Room for a second attempt, so the swept copy is fetched while the first
    // still runs — with one permit it would wait for the first to settle, and
    // the lease would never be asked.
    #[process(queue = StallQueue, retries = 0, concurrency = 2)]
    async fn slow(&self, job: GuardedCommand) -> anyhow::Result<()> {
        run_guarded(&STALLED, job).await;
        Ok(())
    }
}

/// A heartbeat every six seconds, which a peer judging by a threshold of one
/// second takes for a stall; and the guard suites' two-second lease.
fn stalling() -> RedisWorkerConfig {
    RedisWorkerConfig {
        orphan_after: Duration::from_secs(60),
        ..crate::brisk()
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(stalling())],
    providers = [StallProcessor],
)]
struct StallModule;

/// A peer whose orphan threshold the replica's heartbeat fell behind sweeps the
/// replica's in-flight job back onto the queue while it still runs — the sweep
/// is apalis's own, called here as a peer calls it. The replica fetches the
/// swept copy itself; its lease sends that back until the job settles, and the
/// job runs once.
///
/// The job outlasts one heartbeat: against Redis 6.2 a swept worker fetches
/// nothing until its next one (apalis re-registers it on an error text only
/// Redis 7 writes), and the copy has to arrive while the job still runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replica_taken_for_dead_while_it_runs_a_job_never_runs_it_twice() {
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let replica = crate::replica::<StallModule>().await;
    let receipt = replica
        .producer
        .push(StallQueue, GuardedCommand::holding(run, 8000), None)
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || !STALLED.of(run).is_empty()).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // The replica's last heartbeat is more than a second old: a peer judging by
    // a one-second threshold takes it for dead.
    let judged = chrono::Utc::now() - chrono::Duration::seconds(1);
    let swept = apalis_on("nestrs-e2e-lease-stall")
        .await
        .reenqueue_orphaned(10, judged)
        .await
        .expect("a peer's sweep");
    assert!(swept >= 1, "the running job was swept back onto the queue");

    crate::wait_until(Duration::from_secs(20), || {
        said(&logs, SETTLED, receipt.id())
    })
    .await;
    replica.worker.shutdown().await.expect("clean shutdown");

    assert!(
        said(&logs, HANDED_BACK, receipt.id()),
        "the swept copy was handed back while the job ran",
    );
    assert!(
        said(&logs, SETTLED, receipt.id()),
        "and acknowledged without running once the job settled",
    );
    assert_eq!(STALLED.of(run).len(), 1, "the job ran once");
    assert_eq!(STALLED.finished(run), 1, "to its end");
}

// --- a replica dying mid-attempt --------------------------------------------------

static ORPHANED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-lease-death", job = GuardedCommand)]
struct DeathQueue;

#[injectable]
#[derive(Default)]
struct DeathProcessor;

#[processor]
impl DeathProcessor {
    #[process(queue = DeathQueue, retries = 0)]
    async fn slow(&self, job: GuardedCommand) -> anyhow::Result<()> {
        run_guarded(&ORPHANED, job).await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [DeathProcessor],
)]
struct DeathModule;

/// A replica killed mid-attempt — no drain, no hand-back, its lease no longer
/// renewed — leaves its job in flight. The next replica's startup sweep finds
/// it, waits out the dead replica's lease, and runs it: to completion, once
/// overall.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replica_dying_mid_attempt_leaves_its_job_to_complete_exactly_once() {
    let run = crate::this_run();
    let doomed = crate::mortal_replica::<DeathModule>().await;
    doomed
        .producer
        .push(
            DeathQueue,
            GuardedCommand {
                run,
                hold_ms: 60_000,
                once: true,
            },
            None,
        )
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || !ORPHANED.of(run).is_empty()).await;

    doomed.kill().await;
    let heir = crate::replica::<DeathModule>().await;
    crate::wait_until(Duration::from_secs(20), || ORPHANED.finished(run) == 1).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    heir.worker.shutdown().await.expect("clean shutdown");

    assert_eq!(
        ORPHANED.of(run).len(),
        2,
        "started by the replica that died, then by the one after it",
    );
    assert_eq!(ORPHANED.finished(run), 1, "and completed exactly once");
}

// --- one job filed twice ------------------------------------------------------------

static TWICE: Runs = Runs::new();

#[queue(name = "nestrs-e2e-lease-twice", job = GuardedCommand)]
struct TwiceQueue;

#[injectable]
#[derive(Default)]
struct TwiceProcessor;

#[processor]
impl TwiceProcessor {
    #[process(queue = TwiceQueue, retries = 0, concurrency = 2)]
    async fn slow(&self, job: GuardedCommand) -> anyhow::Result<()> {
        run_guarded(&TWICE, job).await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [TwiceProcessor],
)]
struct TwiceModule;

/// Two records carrying one job — the same sealed id — reach one replica with
/// room to run both at once: the second waits on the first's lease, and is
/// acknowledged without running once the first settled the job.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_job_delivered_twice_at_once_runs_once() {
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let id = uuid::Uuid::now_v7().to_string();
    let job = JobId::parse(&id).expect("a v7 id is a job id");
    let record = serde_json::json!({
        "v": nest_rs_queue::WIRE_FORMAT_VERSION,
        "id": id,
        "attempt": 1,
        "payload": GuardedCommand::holding(run, 1500),
    });
    let mut filing = apalis_on("nestrs-e2e-lease-twice").await;
    filing.push(record.clone()).await.expect("first filing");
    filing.push(record).await.expect("second filing");

    let replica = crate::replica::<TwiceModule>().await;
    crate::wait_until(Duration::from_secs(15), || said(&logs, SETTLED, &job)).await;
    replica.worker.shutdown().await.expect("clean shutdown");

    assert!(
        said(&logs, HANDED_BACK, &job),
        "the second delivery waited on the first's lease",
    );
    assert!(
        said(&logs, SETTLED, &job),
        "and was acknowledged without running once the job settled",
    );
    assert_eq!(TWICE.of(run).len(), 1, "the job ran once");
    assert_eq!(TWICE.finished(run), 1, "to its end");
}
