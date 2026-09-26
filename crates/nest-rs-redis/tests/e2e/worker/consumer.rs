//! The transport, against a live worker: what `RedisWorker` owns rather than
//! what one delivery does.
//!
//! **A method's concurrency.** A `#[process]` method runs at most `concurrency`
//! jobs at once on one replica — one unless it declares more. Each bound is
//! proved from both sides: the peak never climbs past it, and it is reached,
//! because a ceiling nobody reaches reads exactly like a worker that serializes
//! everything. The default is the guard on "one" — a wider dispatch or a
//! `tokio::spawn` inside the attempt climbs past it. Only a live worker shows
//! it: the port's suite runs one attempt at a time, which is exactly the
//! condition under which a lost bound stays invisible.
//!
//! **The fetch is exclusive, and every replica is its own consumer.**
//! `get_jobs.lua` claims ids in one atomic EVAL, so two replicas never receive
//! the same job from the queue; and each replica consumes under an id of its
//! own, so its in-flight set is its own.
//!
//! **A shutdown drains within its window**: a running attempt finishes inside
//! it, and one still running as it closes is interrupted and handed back, for a
//! replica that starts later to run.
//!
//! Every test has its own queue and its own counters, since nextest runs them
//! side by side on one Redis.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, processor, queue};
use nest_rs_redis::{
    RedisConnection, RedisModule, RedisQueueModule, RedisQueueProducer, RedisWorkerModule,
};
use nest_rs_testing::LogCapture;
use serde::{Deserialize, Serialize};

use crate::Runs;

/// Simultaneous handler bodies, the peak seen, and the jobs finished — one set
/// per test. Process-wide statics: the container owns the provider, and the
/// assertion runs outside it.
struct Gauge {
    in_flight: AtomicUsize,
    peak: AtomicUsize,
    ran: AtomicUsize,
}

impl Gauge {
    const fn new() -> Self {
        Self {
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            ran: AtomicUsize::new(0),
        }
    }

    /// Hold one slot for `hold`: while it is held, a job the bound lets through
    /// starts beside it and the peak climbs.
    async fn hold(&self, hold: Duration) {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(hold).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.ran.fetch_add(1, Ordering::SeqCst);
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    fn ran(&self) -> usize {
        self.ran.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HoldCommand {
    seq: usize,
}

// --- the default: one at a time ------------------------------------------------

static SERIAL: Gauge = Gauge::new();

#[queue(name = "nestrs-e2e-concurrency-default", job = HoldCommand)]
struct SerialQueue;

#[injectable]
#[derive(Default)]
struct SerialProcessor;

#[processor]
impl SerialProcessor {
    #[process(queue = SerialQueue, retries = 0)]
    async fn hold(&self, _job: HoldCommand) -> anyhow::Result<()> {
        SERIAL.hold(Duration::from_millis(250)).await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [SerialProcessor],
)]
struct SerialModule;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_process_method_runs_one_job_at_a_time_unless_it_declares_more() {
    const JOBS: usize = 6;
    let serial = crate::replica::<SerialModule>().await;
    for seq in 0..JOBS {
        serial
            .producer
            .push(SerialQueue, HoldCommand { seq }, None)
            .await
            .expect("enqueue");
    }
    crate::wait_until(Duration::from_secs(15), || SERIAL.ran() == JOBS).await;
    serial
        .worker
        .shutdown()
        .await
        .expect("clean worker shutdown");

    assert_eq!(SERIAL.ran(), JOBS, "every job drains, one after another");
    assert_eq!(
        SERIAL.peak(),
        1,
        "a method declaring no concurrency runs one job at a time, but {} ran at once",
        SERIAL.peak(),
    );
}

// --- a declared bound ----------------------------------------------------------

static BOUNDED: Gauge = Gauge::new();

#[queue(name = "nestrs-e2e-concurrency-three", job = HoldCommand)]
struct BoundedQueue;

#[injectable]
#[derive(Default)]
struct BoundedProcessor;

#[processor]
impl BoundedProcessor {
    #[process(queue = BoundedQueue, retries = 0, concurrency = 3)]
    async fn hold(&self, _job: HoldCommand) -> anyhow::Result<()> {
        BOUNDED.hold(Duration::from_millis(800)).await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [BoundedProcessor],
)]
struct BoundedModule;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_concurrency_is_reached_and_never_exceeded() {
    const JOBS: usize = 9;
    let bounded = crate::replica::<BoundedModule>().await;
    for seq in 0..JOBS {
        bounded
            .producer
            .push(BoundedQueue, HoldCommand { seq }, None)
            .await
            .expect("enqueue");
    }
    crate::wait_until(Duration::from_secs(15), || BOUNDED.ran() == JOBS).await;
    bounded
        .worker
        .shutdown()
        .await
        .expect("clean worker shutdown");

    assert_eq!(BOUNDED.ran(), JOBS, "every job drains");
    assert_eq!(
        BOUNDED.peak(),
        3,
        "`concurrency = 3` runs three jobs at once and never a fourth",
    );
}

// --- the fetch never hands one job to two replicas ------------------------------

/// Long enough that the batch spreads across both replicas.
const HOLD: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlowCommand {
    seq: usize,
}

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

/// The guarantee that makes replica-based throughput sound: with both replicas
/// already up, a batch is split between them and **no job runs twice**. This is
/// the atomic claim in `get_jobs.lua`, measured rather than read — and, since
/// each replica logs the id it consumes under, the two ids are two.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_fetch_never_hands_one_job_to_two_replicas_and_each_consumes_as_itself() {
    const JOBS: usize = 4;
    let logs = LogCapture::install_global();

    // Both up before any job exists, so no startup sweep can find work in
    // flight — this isolates the fetch from the sweep `lease` covers.
    let first = crate::replica::<FetchModule>().await;
    let second = crate::replica::<FetchModule>().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = RedisQueueProducer::new(
        RedisConnection::connect(&crate::redis_config())
            .await
            .expect("connect"),
    );
    for seq in 0..JOBS {
        conn.push(FetchQueue, SlowCommand { seq }, None)
            .await
            .expect("enqueue");
    }

    // Serialized per replica, two replicas ⇒ ceil(JOBS / 2) waves, plus slack.
    crate::wait_until(
        HOLD * (JOBS as u32).div_ceil(2) + Duration::from_secs(5),
        || FETCH_RUNS.lock().expect("lock").len() >= JOBS,
    )
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    first.worker.shutdown().await.expect("clean shutdown");
    second.worker.shutdown().await.expect("clean shutdown");

    let mut seen = FETCH_RUNS.lock().expect("lock").clone();
    seen.sort_unstable();
    assert_eq!(
        seen,
        (0..JOBS).collect::<Vec<_>>(),
        "every job ran exactly once across the two replicas",
    );
    let mut workers: Vec<String> = logs
        .find(nest_rs_queue::TARGET, "queue worker started")
        .iter()
        .filter(|event| event.field("queue").as_deref() == Some("nestrs-e2e-replicas-fetch"))
        .filter_map(|event| event.field("worker"))
        .collect();
    workers.sort_unstable();
    workers.dedup();
    assert_eq!(
        workers.len(),
        2,
        "two replicas, two consumer ids: {workers:?}"
    );
}

// --- a shutdown drains within its window ----------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DrainCommand {
    run: u64,
    /// How long the first attempt at the job holds on.
    hold_ms: u64,
}

/// Hold on for as long as the job asks — its first attempt only — counting it
/// in `runs`.
async fn hold(runs: &Runs, job: DrainCommand) {
    if runs.start(job.run) == 1 {
        tokio::time::sleep(Duration::from_millis(job.hold_ms)).await;
    }
    runs.finish(job.run);
}

// One queue per test: nextest runs them side by side, and a replica of one test
// fetching the other's job would hold it for the other's hold.

static FINISHED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-drain-inside", job = DrainCommand)]
struct InsideQueue;

#[injectable]
#[derive(Default)]
struct InsideProcessor;

#[processor]
impl InsideProcessor {
    #[process(queue = InsideQueue, retries = 0)]
    async fn hold(&self, job: DrainCommand) -> anyhow::Result<()> {
        hold(&FINISHED, job).await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [InsideProcessor],
)]
struct InsideModule;

static OUTLASTED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-drain-outlast", job = DrainCommand)]
struct OutlastQueue;

#[injectable]
#[derive(Default)]
struct OutlastProcessor;

#[processor]
impl OutlastProcessor {
    #[process(queue = OutlastQueue, retries = 0)]
    async fn hold(&self, job: DrainCommand) -> anyhow::Result<()> {
        hold(&OUTLASTED, job).await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [OutlastProcessor],
)]
struct OutlastModule;

/// An attempt that ends inside the window ends where it runs: the shutdown
/// waits for it, and hands nothing back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_attempt_ending_inside_the_shutdown_window_finishes_where_it_runs() {
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let replica = crate::replica::<InsideModule>().await;
    let receipt = replica
        .producer
        .push(InsideQueue, DrainCommand { run, hold_ms: 400 }, None)
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || !FINISHED.of(run).is_empty()).await;

    replica.worker.shutdown().await.expect("clean shutdown");

    assert_eq!(FINISHED.finished(run), 1, "the attempt ran to its end");
    assert!(
        !logs
            .find(nest_rs_queue::TARGET, "job handed back to the queue")
            .iter()
            .any(|event| crate::names(event, receipt.id())),
        "nothing was handed back",
    );
}

/// An attempt still running as the window closes is interrupted and its job
/// handed back inside the window — the shutdown never outlasts it — and the
/// replica that starts next runs the job, once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_attempt_outlasting_the_shutdown_window_is_handed_back_within_it() {
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let first = crate::replica::<OutlastModule>().await;
    let receipt = first
        .producer
        .push(
            OutlastQueue,
            DrainCommand {
                run,
                hold_ms: 60_000,
            },
            None,
        )
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || !OUTLASTED.of(run).is_empty()).await;

    let stopping = Instant::now();
    first.worker.shutdown().await.expect("clean shutdown");
    let took = stopping.elapsed();
    assert!(
        took < crate::brisk().shutdown_timeout + Duration::from_millis(500),
        "the drain kept within its window, not {took:?}",
    );
    let handed_back = logs.find(nest_rs_queue::TARGET, "job handed back to the queue");
    let handed_back = handed_back
        .iter()
        .find(|event| crate::names(event, receipt.id()))
        .unwrap_or_else(|| panic!("the job was handed back, not dropped: {handed_back:#?}"));
    assert_eq!(handed_back.field("reason").as_deref(), Some("shutdown"));
    assert_eq!(
        OUTLASTED.finished(run),
        0,
        "the interrupted attempt never reached its end"
    );

    let second = crate::replica::<OutlastModule>().await;
    crate::wait_until(Duration::from_secs(15), || OUTLASTED.finished(run) == 1).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    second.worker.shutdown().await.expect("clean shutdown");

    assert_eq!(
        OUTLASTED.of(run).len(),
        2,
        "interrupted once, then run by the next replica"
    );
    assert_eq!(OUTLASTED.finished(run), 1, "and completed exactly once");
}
