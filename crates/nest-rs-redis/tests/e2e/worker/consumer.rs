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
//! **One poll fills every permit, and a busy replica holds little.** A fetch
//! takes up to the method's `concurrency`, so `concurrency` jobs waiting start
//! together rather than one poll apart; and a replica whose permits are taken
//! keeps at most `concurrency` jobs it fetched and has not started, counted in
//! apalis's own in-flight set.
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
    RedisConnection, RedisModule, RedisQueueModule, RedisQueueProducer, RedisWorkerConfig,
    RedisWorkerModule,
};
use nest_rs_testing::LogCapture;
use serde::{Deserialize, Serialize};

use crate::Runs;

/// Simultaneous handler bodies, the peak seen, when each started, and the jobs
/// finished — one set per test. Process-wide statics: the container owns the
/// provider, and the assertion runs outside it.
struct Gauge {
    in_flight: AtomicUsize,
    peak: AtomicUsize,
    ran: AtomicUsize,
    started: Mutex<Vec<Instant>>,
}

impl Gauge {
    const fn new() -> Self {
        Self {
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            ran: AtomicUsize::new(0),
            started: Mutex::new(Vec::new()),
        }
    }

    /// Hold one slot for `hold`: while it is held, a job the bound lets through
    /// starts beside it and the peak climbs.
    async fn hold(&self, hold: Duration) {
        self.started
            .lock()
            .expect("gauge lock")
            .push(Instant::now());
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

    /// When each job started, in order.
    fn started(&self) -> Vec<Instant> {
        self.started.lock().expect("gauge lock").clone()
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

/// A poll long enough that jobs fetched one poll apart could never pass for
/// jobs fetched together.
const SLOW_POLL: Duration = Duration::from_millis(400);

fn slow_poll() -> RedisWorkerConfig {
    RedisWorkerConfig {
        poll_interval: SLOW_POLL,
        ..Default::default()
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(slow_poll())],
    providers = [BoundedProcessor],
)]
struct BoundedModule;

/// Nine jobs wait before the worker starts: its first poll takes three — one
/// fetch filling every permit, so the three start together, not a poll apart —
/// and no fourth ever runs beside them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_concurrency_is_reached_in_one_fetch_and_never_exceeded() {
    const JOBS: usize = 9;
    let producer = crate::producer().await;
    for seq in 0..JOBS {
        producer
            .push(BoundedQueue, HoldCommand { seq }, None)
            .await
            .expect("enqueue");
    }
    let bounded = crate::replica::<BoundedModule>().await;
    crate::wait_until(Duration::from_secs(20), || BOUNDED.ran() == JOBS).await;
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
    let started = BOUNDED.started();
    let first = &started[..3];
    let spread = first[2] - first[0];
    assert!(
        spread < SLOW_POLL / 2,
        "the three jobs waiting started from one fetch, not one poll apart: {spread:?} between the \
         first and the third, at a poll of {SLOW_POLL:?}",
    );
}

// --- a busy replica holds at most its concurrency ------------------------------

static HELD: Gauge = Gauge::new();

/// The concurrency the holding test declares.
const HOLDING: usize = 4;

#[queue(name = "nestrs-e2e-concurrency-held", job = HoldCommand)]
struct HeldQueue;

#[injectable]
#[derive(Default)]
struct HeldProcessor;

#[processor]
impl HeldProcessor {
    #[process(queue = HeldQueue, retries = 0, concurrency = 4)]
    async fn hold(&self, job: HoldCommand) -> anyhow::Result<()> {
        // Staggered, so the replica is mostly one permit short of full: each
        // poll then fetches a whole batch for a single free permit — the case
        // that holds the most jobs unstarted.
        let hold = 200 + 150 * (job.seq % HOLDING) as u64;
        HELD.hold(Duration::from_millis(hold)).await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [HeldProcessor],
)]
struct HeldModule;

/// How many jobs apalis holds in flight for `queue` right now, across every
/// replica consuming it — each consumer's in-flight set, summed in one script
/// so no fetch lands between two reads. Read straight from apalis's keys: the
/// bound is apalis's to keep, and this is where apalis keeps it.
async fn in_flight(conn: &mut nest_rs_redis::RedisConnection, queue: &str) -> usize {
    redis::Script::new(
        r"
local total = 0
for _, set in ipairs(redis.call('ZRANGE', KEYS[1], 0, -1)) do
  total = total + redis.call('SCARD', set)
end
return total
",
    )
    .key(format!("{}:consumers", crate::namespace(queue)))
    .invoke_async(conn)
    .await
    .expect("the in-flight count")
}

/// Sixteen jobs of four permits' worth: apalis's in-flight set — the jobs a
/// replica fetched and has not settled — never holds more than twice the
/// method's concurrency, the ones running and at most as many fetched ahead of a
/// free permit; and it does hold some ahead, which is what the bound is about.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_busy_replica_holds_at_most_its_concurrency_in_jobs_it_has_not_started() {
    const JOBS: usize = 16;
    let producer = crate::producer().await;
    for seq in 0..JOBS {
        producer
            .push(HeldQueue, HoldCommand { seq }, None)
            .await
            .expect("enqueue");
    }
    let replica = crate::replica::<HeldModule>().await;
    let mut conn = crate::connect().await;
    let mut most = 0;
    let deadline = Instant::now() + Duration::from_secs(20);
    while HELD.ran() < JOBS && Instant::now() < deadline {
        most = most.max(in_flight(&mut conn, "nestrs-e2e-concurrency-held").await);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    replica
        .worker
        .shutdown()
        .await
        .expect("clean worker shutdown");

    assert_eq!(HELD.ran(), JOBS, "every job drains");
    assert_eq!(HELD.peak(), HOLDING, "four at once, never a fifth");
    assert!(
        most <= 2 * HOLDING,
        "a busy replica held {most} jobs in flight — more than the {HOLDING} it runs and the \
         {HOLDING} it may fetch ahead",
    );
    assert!(
        most > HOLDING,
        "a fetch for one free permit brought a batch, held ahead of the permits: at most {most} \
         jobs were in flight",
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
