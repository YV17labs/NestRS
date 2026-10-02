//! The delivery guard, against live workers: a job apalis delivers twice runs
//! once, a job a cancel reached first never runs, a unique key goes with its
//! job's outcome, and a throttle caps how many attempts start per window.
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
//! In each case the guard meets the second delivery the same way: it is handed
//! back while the first holds the lease, and acknowledged without running once
//! the first has settled the job — inside the settled mark's one span, which
//! every test here stays within.
//!
//! The same step meets a cancel: a job cancelled while it waits — on its queue,
//! on its delay, or for its next attempt — is acknowledged without running when
//! it is delivered, and a cancel while an attempt runs is refused and the job
//! runs to its end. Settling a job lets go of its unique key, whether it
//! completed or dead-lettered. And a method's throttle counts every start across
//! every replica: an attempt over the limit waits for the window to end, neither
//! counted as an attempt nor dropped.
//!
//! The guard is a filter, not a promise of exactly once: a settled mark is kept
//! for one fixed span, whatever happened around it, so a redelivery later than
//! that — an acknowledgement apalis dropped, then a long quiet — runs the job
//! again, as at least once allows. A lost acknowledgement is said, and changes
//! nothing about the mark.
//!
//! Replicas here run [`brisk`](crate::brisk) settings: a two-second lease, so a
//! lease a dead replica held is free again within the test.

use std::time::{Duration, Instant};

use apalis::prelude::Storage;
use apalis_redis::{Config, RedisStorage};
use nest_rs_core::{injectable, module, operation_log};
use nest_rs_queue::{
    JobError, JobId, JobProducerExt, PushOptions, PushReceipt, QueueError, processor, queue, unit,
};
use nest_rs_redis::{
    RedisConnection, RedisModule, RedisQueueModule, RedisQueueProducer, RedisWorkerConfig,
    RedisWorkerModule,
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
async fn a_replica_starting_mid_flight_hands_the_swept_job_back_then_acknowledges_it_without_running()
 {
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
/// settled mark then acknowledges it without running.
///
/// The job outlasts one heartbeat: against Redis 6.2 a swept worker fetches
/// nothing until its next one (apalis re-registers it on an error text only
/// Redis 7 writes), and the copy has to arrive while the job still runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replica_taken_for_dead_while_it_runs_a_job_hands_the_swept_copy_back_then_acknowledges_it_without_running()
 {
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
    /// One retry: the attempt that dies with its replica spends the first.
    #[process(queue = DeathQueue, retries = 1)]
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
/// it, waits out the dead replica's lease, and runs it again: the dead
/// replica's attempt started it, the next one completes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replica_dying_mid_attempt_leaves_its_job_to_the_next_replica_after_its_lease() {
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
    assert_eq!(
        ORPHANED.finished(run),
        1,
        "and completed by the replica after it"
    );
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
async fn one_job_delivered_twice_at_once_hands_the_second_delivery_back_then_acknowledges_it_without_running()
 {
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

// --- a unique key goes with its job's outcome ---------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OutcomeCommand {
    run: u64,
    /// Whether the attempt fails for good.
    fail: bool,
}

static OUTCOMES: Runs = Runs::new();

#[queue(name = "nestrs-e2e-unique-outcome", job = OutcomeCommand)]
struct OutcomeQueue;

#[injectable]
#[derive(Default)]
struct OutcomeProcessor;

#[processor]
impl OutcomeProcessor {
    #[process(queue = OutcomeQueue, retries = 0)]
    async fn run(&self, job: OutcomeCommand) -> Result<(), JobError> {
        OUTCOMES.start(job.run);
        if job.fail {
            return Err(JobError::abort(
                "the payload names a file that does not exist",
            ));
        }
        OUTCOMES.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [OutcomeProcessor],
)]
struct OutcomeModule;

/// Push `job` under `key` as soon as the key is free, within `within`: the job
/// holding it lets go when its delivery settles it, a moment after its handler
/// returns.
async fn push_when_free(
    producer: &RedisQueueProducer,
    job: OutcomeCommand,
    key: &str,
    within: Duration,
) -> PushReceipt {
    let deadline = Instant::now() + within;
    loop {
        match producer
            .push(
                OutcomeQueue,
                job.clone(),
                PushOptions::default().with_unique(key),
            )
            .await
        {
            Ok(receipt) => return receipt,
            Err(QueueError::UniqueKeyHeld { .. }) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(other) => panic!("the key was never let go: {other}"),
        }
    }
}

/// A unique key is held while its job waits and runs, and free for the next
/// push once the job completes — and again once a job under it dead-letters.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_unique_push_key_is_let_go_when_its_job_completes_and_when_it_dead_letters() {
    let run = crate::this_run();
    let key = format!("outcome-{run}");
    let replica = crate::replica::<OutcomeModule>().await;

    let completing = replica
        .producer
        .push(
            OutcomeQueue,
            OutcomeCommand { run, fail: false },
            PushOptions::default().with_unique(key.as_str()),
        )
        .await
        .expect("the first job under the key");
    let doomed = push_when_free(
        &replica.producer,
        OutcomeCommand {
            run: run + 1,
            fail: true,
        },
        &key,
        Duration::from_secs(10),
    )
    .await;
    assert_eq!(OUTCOMES.finished(run), 1, "the first job completed first");
    let last = push_when_free(
        &replica.producer,
        OutcomeCommand {
            run: run + 2,
            fail: false,
        },
        &key,
        Duration::from_secs(10),
    )
    .await;
    assert_eq!(
        OUTCOMES.of(run + 1).len(),
        1,
        "the second job ran and dead-lettered before the key was free"
    );
    assert_eq!(OUTCOMES.finished(run + 1), 0);
    crate::wait_until(Duration::from_secs(10), || OUTCOMES.finished(run + 2) == 1).await;
    replica.worker.shutdown().await.expect("clean shutdown");

    assert_ne!(completing.id(), doomed.id());
    assert_eq!(
        OUTCOMES.finished(run + 2),
        1,
        "the third job ran under it too"
    );
    let claim = crate::key_of("nestrs-e2e-unique-outcome", "unique", &key);
    let open = crate::key_of("nestrs-e2e-unique-outcome", "open", &last.id().to_string());
    let deadline = Instant::now() + Duration::from_secs(5);
    while crate::read(&claim).await.is_some() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        crate::read(&claim).await,
        None,
        "and the last job let go of it"
    );
    assert_eq!(
        crate::read(&open).await,
        None,
        "its open record closed with it"
    );
}

// --- a cancel met by the delivery ------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CancelCommand {
    run: u64,
    /// How long each attempt holds on.
    hold_ms: u64,
    /// Whether the first attempt fails, retryably.
    fail_first: bool,
}

impl CancelCommand {
    fn plain(run: u64) -> Self {
        Self {
            run,
            hold_ms: 0,
            fail_first: false,
        }
    }
}

/// Run a cancel suite's job: count its start, fail the first attempt when it
/// says so, hold on, count its end.
async fn run_cancellable(runs: &Runs, job: CancelCommand) -> anyhow::Result<()> {
    let attempt = runs.start(job.run);
    if job.fail_first && attempt == 1 {
        anyhow::bail!("the upstream timed out");
    }
    tokio::time::sleep(Duration::from_millis(job.hold_ms)).await;
    runs.finish(job.run);
    Ok(())
}

// Each suite drains a queue of its own: nextest runs every test in a process of
// its own, and a worker in one would take another's jobs.

static WAITED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-cancel-waiting", job = CancelCommand)]
struct WaitingCancelQueue;

#[injectable]
#[derive(Default)]
struct WaitingCancelProcessor;

#[processor]
impl WaitingCancelProcessor {
    #[process(queue = WaitingCancelQueue, retries = 2)]
    async fn run(&self, job: CancelCommand) -> anyhow::Result<()> {
        run_cancellable(&WAITED, job).await
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [WaitingCancelProcessor],
)]
struct WaitingCancelModule;

static RUNNING: Runs = Runs::new();

#[queue(name = "nestrs-e2e-cancel-running", job = CancelCommand)]
struct RunningCancelQueue;

#[injectable]
#[derive(Default)]
struct RunningCancelProcessor;

#[processor]
impl RunningCancelProcessor {
    #[process(queue = RunningCancelQueue, retries = 2)]
    async fn run(&self, job: CancelCommand) -> anyhow::Result<()> {
        run_cancellable(&RUNNING, job).await
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [RunningCancelProcessor],
)]
struct RunningCancelModule;

static RETRIED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-cancel-retry", job = CancelCommand)]
struct RetryCancelQueue;

#[injectable]
#[derive(Default)]
struct RetryCancelProcessor;

#[processor]
impl RetryCancelProcessor {
    #[process(queue = RetryCancelQueue, retries = 2)]
    async fn run(&self, job: CancelCommand) -> anyhow::Result<()> {
        run_cancellable(&RETRIED, job).await
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [RetryCancelProcessor],
)]
struct RetryCancelModule;

/// The line a delivery files for a job a cancel reached first.
const CANCELLED_LINE: &str = "job delivered after it was cancelled; acknowledged without running";

/// A job cancelled while it waited on its queue, and one cancelled while its
/// delay held it back, are delivered once a worker runs and they are due — and
/// acknowledged without running, the cancel's `true` kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_of_a_waiting_or_delayed_job_keeps_it_from_ever_running() {
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let producer = crate::producer().await;
    let waiting = producer
        .push(WaitingCancelQueue, CancelCommand::plain(run), None)
        .await
        .expect("a job waiting for a worker");
    let delayed = producer
        .push(
            WaitingCancelQueue,
            CancelCommand::plain(run + 1),
            PushOptions::default().with_delay(Duration::from_secs(2)),
        )
        .await
        .expect("a delayed job");
    assert!(producer.cancel(&waiting).await.expect("a cancel"));
    assert!(producer.cancel(&delayed).await.expect("a cancel"));

    let replica = crate::replica::<WaitingCancelModule>().await;
    crate::wait_until(Duration::from_secs(15), || {
        said(&logs, CANCELLED_LINE, waiting.id()) && said(&logs, CANCELLED_LINE, delayed.id())
    })
    .await;
    replica.worker.shutdown().await.expect("clean shutdown");

    for receipt in [&waiting, &delayed] {
        assert!(
            said(&logs, CANCELLED_LINE, receipt.id()),
            "delivered, and acknowledged without running: {}",
            receipt.id(),
        );
    }
    assert!(WAITED.of(run).is_empty(), "the waiting job never ran");
    assert!(WAITED.of(run + 1).is_empty(), "the delayed job never ran");
}

/// A cancel while an attempt runs is refused — the job started — and the job
/// runs to its end; once it has, a cancel still answers `false`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_while_an_attempt_runs_is_refused_and_the_job_runs_to_its_end() {
    let run = crate::this_run();
    let replica = crate::replica::<RunningCancelModule>().await;
    let receipt = replica
        .producer
        .push(
            RunningCancelQueue,
            CancelCommand {
                hold_ms: 1500,
                ..CancelCommand::plain(run)
            },
            None,
        )
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || !RUNNING.of(run).is_empty()).await;

    assert!(
        !replica.producer.cancel(&receipt).await.expect("a cancel"),
        "an attempt runs, so the job is not cancelled",
    );
    crate::wait_until(Duration::from_secs(10), || RUNNING.finished(run) == 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !replica.producer.cancel(&receipt).await.expect("a cancel"),
        "a finished job is not cancelled either",
    );
    replica.worker.shutdown().await.expect("clean shutdown");
    assert_eq!(RUNNING.of(run).len(), 1);
    assert_eq!(RUNNING.finished(run), 1, "it ran to its end");
}

/// A job whose attempt failed waits on the schedule for its next one; a cancel
/// there is a cancel of a waiting job — `true`, and the next attempt never runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_of_a_job_waiting_for_its_retry_keeps_it_from_its_next_attempt() {
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let replica = crate::replica::<RetryCancelModule>().await;
    let receipt = replica
        .producer
        .push(
            RetryCancelQueue,
            CancelCommand {
                fail_first: true,
                ..CancelCommand::plain(run)
            },
            None,
        )
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || !RETRIED.of(run).is_empty()).await;

    // The first attempt's delivery files the next one, then lets go of the job:
    // from then until the next attempt starts, the job waits.
    let deadline = Instant::now() + Duration::from_millis(700);
    let mut cancelled = false;
    while !cancelled && Instant::now() < deadline {
        cancelled = replica.producer.cancel(&receipt).await.expect("a cancel");
        if !cancelled {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    assert!(cancelled, "the job waiting for its retry was cancelled");
    crate::wait_until(Duration::from_secs(10), || {
        said(&logs, CANCELLED_LINE, receipt.id())
    })
    .await;
    replica.worker.shutdown().await.expect("clean shutdown");

    assert!(said(&logs, CANCELLED_LINE, receipt.id()));
    assert_eq!(
        RETRIED.of(run).len(),
        1,
        "the first attempt ran, and the next never did"
    );
}

// --- a throttle across replicas ----------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ThrottledCommand {
    run: u64,
    take: u64,
}

static THROTTLED: Runs = Runs::new();

/// The window the throttled method declares.
const THROTTLE_WINDOW: Duration = Duration::from_secs(2);

#[queue(name = "nestrs-e2e-throttle", job = ThrottledCommand)]
struct ThrottledQueue;

#[injectable]
#[derive(Default)]
struct ThrottledProcessor;

#[processor]
impl ThrottledProcessor {
    #[process(queue = ThrottledQueue, concurrency = 4, throttle(limit = 2, window = "2s"))]
    async fn run(&self, job: ThrottledCommand) -> anyhow::Result<()> {
        THROTTLED.start(job.run.wrapping_add(1 + job.take));
        THROTTLED.start(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [ThrottledProcessor],
)]
struct ThrottledModule;

/// Two replicas with four permits each could start six jobs at once; a
/// throttle of two per two-second window, counted in Redis, lets two start in
/// the first window and holds the rest back for the windows after — every job
/// runs once, on its first attempt, and none is dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_throttle_caps_attempt_starts_per_window_across_replicas_and_defers_the_rest() {
    /// How far a start may land before the window it opened by Redis's clock, as
    /// read by this process's — the time between the counting step and the
    /// handler's first line.
    const SLACK: Duration = Duration::from_millis(250);
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    // A window an earlier run of this suite opened would count this run's first
    // starts, and the first window has to open with this run's first job.
    let _: i64 = redis::cmd("DEL")
        .arg(format!(
            "{}:throttle",
            crate::namespace("nestrs-e2e-throttle")
        ))
        .query_async(&mut crate::connect().await)
        .await
        .expect("DEL");
    let first = crate::replica::<ThrottledModule>().await;
    let second = crate::replica::<ThrottledModule>().await;
    let mut receipts = Vec::new();
    for take in 0..6 {
        receipts.push(
            first
                .producer
                .push(ThrottledQueue, ThrottledCommand { run, take }, None)
                .await
                .expect("enqueue"),
        );
    }
    crate::wait_until(Duration::from_secs(30), || THROTTLED.of(run).len() == 6).await;
    first.worker.shutdown().await.expect("clean shutdown");
    second.worker.shutdown().await.expect("clean shutdown");

    for take in 0..6 {
        assert_eq!(
            THROTTLED.of(run.wrapping_add(1 + take)).len(),
            1,
            "job {take} ran once, never dropped"
        );
    }
    let mut started = THROTTLED.of(run);
    started.sort();
    assert!(
        started[2] - started[0] + SLACK >= THROTTLE_WINDOW,
        "the first window let two jobs start, not three: {:?}",
        started[2] - started[0],
    );
    assert!(
        started[5] - started[0] + SLACK >= THROTTLE_WINDOW * 2,
        "six jobs at two a window took three windows, not {:?}",
        started[5] - started[0],
    );
    let lines: Vec<_> = logs
        .find(operation_log::TARGET, unit::JOB.name())
        .into_iter()
        .filter(|line| {
            receipts
                .iter()
                .any(|receipt| crate::names(line, receipt.id()))
        })
        .collect();
    assert_eq!(lines.len(), 6, "one attempt per job: {lines:#?}");
    assert!(
        lines
            .iter()
            .all(|line| line.field("attempt").as_deref() == Some("1")
                && line.field("outcome").as_deref() == Some(operation_log::OK)),
        "a job held back by the throttle keeps its first attempt: {lines:#?}",
    );
    assert!(
        !logs
            .find(
                nest_rs_queue::TARGET,
                "job deferred to its throttle's next window"
            )
            .is_empty(),
        "the jobs over the limit were deferred",
    );
}

// --- a lost acknowledgement ----------------------------------------------------------

/// The one span every settled mark is kept for under [`brisk`](crate::brisk)
/// settings: the hour floor, since two orphan thresholds and a lease are seconds.
const SETTLED_FOR_MS: i64 = 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AckCommand {
    run: u64,
}

static ACKED: Runs = Runs::new();

/// Let the waiting attempt end.
static LET_GO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const UNACKED_QUEUE: &str = "nestrs-e2e-lease-ack-lost";

#[queue(name = "nestrs-e2e-lease-ack-lost", job = AckCommand)]
struct UnackedQueue;

#[injectable]
#[derive(Default)]
struct UnackedProcessor;

#[processor]
impl UnackedProcessor {
    #[process(queue = UnackedQueue, retries = 0)]
    async fn run(&self, job: AckCommand) -> anyhow::Result<()> {
        ACKED.start(job.run);
        while !LET_GO.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        ACKED.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [UnackedProcessor],
)]
struct UnackedModule;

/// apalis loses an acknowledgement while the replica runs on — here because the
/// record it reads back to acknowledge is gone — and the job then stays in the
/// flight of a live replica, which no periodic sweep takes, until a replica
/// sweeps it. The line says so at `error`, and the job's settled mark keeps the
/// one span every mark is kept for: nothing extends it, so a redelivery after
/// it lapses runs the job again, which at least once allows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_acknowledgement_is_said_and_the_settled_mark_keeps_its_one_span() {
    crate::forget(UNACKED_QUEUE).await;
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let replica = crate::replica::<UnackedModule>().await;
    let receipt = replica
        .producer
        .push(UnackedQueue, AckCommand { run }, None)
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || !ACKED.of(run).is_empty()).await;

    let apalis = Config::default().set_namespace(&crate::namespace(UNACKED_QUEUE));
    let _: i64 = redis::cmd("DEL")
        .arg(apalis.job_data_hash())
        .query_async(&mut crate::connect().await)
        .await
        .expect("take away the record apalis acknowledges by");
    LET_GO.store(true, std::sync::atomic::Ordering::SeqCst);

    let lost = "job acknowledgement lost; the job stays in flight until a replica sweeps it, and \
                runs again then if its settled mark has lapsed";
    let said_for_this_queue = || {
        logs.find(nest_rs_queue::TARGET, lost)
            .into_iter()
            .filter(|event| event.field("queue").as_deref() == Some(UNACKED_QUEUE))
            .collect::<Vec<_>>()
    };
    crate::wait_until(Duration::from_secs(10), || {
        !said_for_this_queue().is_empty()
    })
    .await;
    let mark = crate::key_of(UNACKED_QUEUE, "settled", &receipt.id().to_string());
    let left = crate::pttl(&mark).await;
    let said = said_for_this_queue();
    replica.worker.shutdown().await.expect("clean shutdown");
    crate::forget(UNACKED_QUEUE).await;

    assert_eq!(ACKED.finished(run), 1, "the job completed");
    assert!(
        left > 0 && left <= SETTLED_FOR_MS,
        "the mark keeps the one span every mark is kept for, not {left} ms",
    );
    assert_eq!(said.len(), 1, "the lost acknowledgement is said: {said:#?}");
    assert_eq!(said[0].level, "error");
}

// --- a throttled method stops fetching until its window ends -----------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BacklogCommand {
    run: u64,
}

static BACKLOG: Runs = Runs::new();

const BACKLOG_QUEUE: &str = "nestrs-e2e-lease-throttle-backlog";

/// The throttled method's permits, and so the most one fetch brings.
const BACKLOG_CONCURRENCY: usize = 4;

#[queue(name = "nestrs-e2e-lease-throttle-backlog", job = BacklogCommand)]
struct BacklogQueue;

#[injectable]
#[derive(Default)]
struct BacklogProcessor;

#[processor]
impl BacklogProcessor {
    #[process(queue = BacklogQueue, concurrency = 4, throttle(limit = 1, window = "2s"))]
    async fn run(&self, job: BacklogCommand) -> anyhow::Result<()> {
        BACKLOG.start(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [BacklogProcessor],
)]
struct BacklogModule;

/// A backlog far past the limit waits on the queue while the window is full:
/// the first refusal shuts the method's fetch until the window ends, so each
/// window costs the replica one fetch's worth of refusals, whatever the backlog.
/// Fetching on, the replica cycled the backlog through admission and the
/// schedule — some forty refusals a second here, twenty-odd Redis calls each —
/// to start one job a window all the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_throttled_backlog_waits_on_the_queue_and_costs_a_window_one_fetch_of_refusals() {
    const BACKLOG_JOBS: usize = 200;
    const WATCHED: Duration = Duration::from_secs(5);
    crate::forget(BACKLOG_QUEUE).await;
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let producer = crate::producer().await;
    let jobs: Vec<BacklogCommand> = (0..BACKLOG_JOBS).map(|_| BacklogCommand { run }).collect();
    producer
        .push_many(BacklogQueue, jobs, None)
        .await
        .expect("the backlog");

    let replica = crate::replica::<BacklogModule>().await;
    tokio::time::sleep(WATCHED).await;
    let waiting = crate::waiting(BACKLOG_QUEUE).await;
    replica.worker.shutdown().await.expect("clean shutdown");
    crate::forget(BACKLOG_QUEUE).await;

    let refused = logs
        .find(
            nest_rs_queue::TARGET,
            "job deferred to its throttle's next window",
        )
        .into_iter()
        .filter(|event| event.field("queue").as_deref() == Some(BACKLOG_QUEUE))
        .count();
    let started = BACKLOG.of(run).len();
    assert!(
        (2..=4).contains(&started),
        "one start a window of two seconds, not {started}"
    );
    let windows = WATCHED.as_secs() as usize / 2 + 1;
    assert!(
        refused <= windows * BACKLOG_CONCURRENCY,
        "at most one fetch of refusals a window — {windows} windows of {BACKLOG_CONCURRENCY} — \
         not {refused}",
    );
    // Apart from the jobs started, one fetch refused and filed for the window's
    // end, and one fetched and held for it.
    let elsewhere = started + 2 * BACKLOG_CONCURRENCY;
    let unstarted = i64::try_from(BACKLOG_JOBS - elsewhere).unwrap_or(0);
    assert!(
        waiting >= unstarted,
        "the backlog waits on the queue, where peers and an autoscaler see it: {waiting} of \
         {BACKLOG_JOBS}",
    );
}
