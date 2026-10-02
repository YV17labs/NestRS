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
//! **Due records reach the queue at the fetch's pace.** A burst of records
//! falling due at once — retries, hand-backs — is moved onto `active` a hundred
//! a second at the least, by the worker's own promotion, where apalis's scan
//! moved one fetch's worth: one record a second at the default concurrency.
//!
//! **A fetch Redis answers late still delivers what it claimed.** The fetch
//! claims ids into the replica's flight in the script it answers with, so a
//! wait cut at the connection's budget would strand them; it waits instead.
//!
//! Every test has its own queue and its own counters, since nextest runs them
//! side by side on one Redis — and counts the jobs of its own run and no other.
//! Queue names are compile-time literals, so an earlier run killed mid-job
//! leaves work a starting replica's sweep hands to this one; each test that
//! counts starts beside such a job ([`crate::ghost`]), so a count that took it
//! in fails here rather than on the next unlucky run.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use apalis::prelude::{Request, Storage};
use apalis_redis::{Config, RedisStorage};

use nest_rs_core::{injectable, module};
use nest_rs_queue::{JobProducerExt, processor, queue};
use nest_rs_redis::{
    RedisConfig, RedisConnection, RedisModule, RedisQueueModule, RedisQueueProducer,
    RedisWorkerConfig, RedisWorkerModule,
};
use nest_rs_testing::LogCapture;
use serde::{Deserialize, Serialize};

use crate::Runs;

/// Simultaneous handler bodies, the peak seen, when each started, and the jobs
/// finished — one set per test, counting the jobs of the run it began and no
/// other. Process-wide statics: the container owns the provider, and the
/// assertion runs outside it.
struct Gauge {
    run: AtomicU64,
    in_flight: AtomicUsize,
    peak: AtomicUsize,
    ran: AtomicUsize,
    started: Mutex<Vec<Instant>>,
}

impl Gauge {
    const fn new() -> Self {
        Self {
            run: AtomicU64::new(0),
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            ran: AtomicUsize::new(0),
            started: Mutex::new(Vec::new()),
        }
    }

    /// Count the jobs of `run` from here on, and only those.
    fn begin(&self, run: u64) {
        self.run.store(run, Ordering::SeqCst);
    }

    /// Hold one slot for `hold` when the job is of the run this gauge counts:
    /// while it is held, a job the bound lets through starts beside it and the
    /// peak climbs. A job of any other run — one an earlier run left in flight,
    /// which a sweep hands this one — returns at once, uncounted.
    async fn hold(&self, run: u64, hold: Duration) {
        if run != self.run.load(Ordering::SeqCst) {
            return;
        }
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
    run: u64,
}

/// A job an earlier run of the suite left in flight on `queue`, under a
/// consumer long silent — what every test counting its jobs starts beside —
/// and nothing else of that run.
///
/// The queue is cleared first: the ghost an earlier run planted may still be
/// waiting on `active` when that run's worker stopped, at the head of the list,
/// and the first fetch would take it before this run's jobs — one fewer of
/// them than the test declares waiting.
async fn ghost_of_an_earlier_run(queue: &str) {
    crate::forget(queue).await;
    crate::ghost(queue, serde_json::json!({ "seq": 0, "run": 0 })).await;
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
    async fn hold(&self, job: HoldCommand) -> anyhow::Result<()> {
        SERIAL.hold(job.run, Duration::from_millis(250)).await;
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
    let run = crate::this_run();
    SERIAL.begin(run);
    ghost_of_an_earlier_run("nestrs-e2e-concurrency-default").await;
    let serial = crate::replica::<SerialModule>().await;
    for seq in 0..JOBS {
        serial
            .producer
            .push(SerialQueue, HoldCommand { seq, run }, None)
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
    async fn hold(&self, job: HoldCommand) -> anyhow::Result<()> {
        BOUNDED.hold(job.run, Duration::from_millis(800)).await;
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
    let run = crate::this_run();
    BOUNDED.begin(run);
    ghost_of_an_earlier_run("nestrs-e2e-concurrency-three").await;
    let producer = crate::producer().await;
    for seq in 0..JOBS {
        producer
            .push(BoundedQueue, HoldCommand { seq, run }, None)
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
        HELD.hold(job.run, Duration::from_millis(hold)).await;
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
/// so no fetch lands between two reads, a ghost's aside. Read straight from
/// apalis's keys: the bound is apalis's to keep, and this is where apalis keeps
/// it.
async fn in_flight(conn: &mut nest_rs_redis::RedisConnection, queue: &str) -> usize {
    redis::Script::new(
        r"
local total = 0
for _, set in ipairs(redis.call('ZRANGE', KEYS[1], 0, -1)) do
  if not string.find(set, ':ghost-', 1, true) then
    total = total + redis.call('SCARD', set)
  end
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
    let run = crate::this_run();
    HELD.begin(run);
    ghost_of_an_earlier_run("nestrs-e2e-concurrency-held").await;
    let producer = crate::producer().await;
    for seq in 0..JOBS {
        producer
            .push(HeldQueue, HoldCommand { seq, run }, None)
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

// --- the fetch's ceiling rests on Lua's -----------------------------------------

/// apalis hands a fetch's ids to Redis in one Lua `unpack` — `sadd`, `hmget` and
/// `rpush` take them all as arguments — and a sweep hands over ten fetches' worth
/// the same way, so the worker sizes a fetch for ten of them to fit: 7,999
/// values in one call, the shape apalis's scripts use. Pinned on the Redis the
/// suite runs against, since a release that moved the limit moves the ceiling.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redis_lua_unpacks_the_values_a_fetch_is_sized_under_and_no_more() {
    let mut conn = crate::connect().await;
    let key = crate::unique_key("lua-unpack");
    let unpack = redis::Script::new(
        r"
local ids = {}
for i = 1, tonumber(ARGV[1]) do
  ids[i] = i
end
local pushed = redis.call('rpush', KEYS[1], unpack(ids))
redis.call('del', KEYS[1])
return pushed
",
    );
    let fits: i64 = unpack
        .key(&key)
        .arg(7_999)
        .invoke_async(&mut conn)
        .await
        .expect("7,999 values unpack into one call");
    assert_eq!(fits, 7_999);
    let refused = unpack
        .key(&key)
        .arg(8_000)
        .invoke_async::<i64>(&mut conn)
        .await
        .expect_err("8,000 do not");
    assert!(
        refused.to_string().contains("too many results to unpack"),
        "{refused}"
    );
}

// --- the fetch never hands one job to two replicas ------------------------------

/// Long enough that the batch spreads across both replicas.
const HOLD: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SlowCommand {
    seq: usize,
    run: u64,
}

/// Every job that ran, by the run that pushed it.
static FETCH_RUNS: Mutex<Vec<(u64, usize)>> = Mutex::new(Vec::new());

/// The jobs of `run` that ran, in the order they did.
fn fetched_of(run: u64) -> Vec<usize> {
    FETCH_RUNS
        .lock()
        .expect("lock")
        .iter()
        .filter(|(of, _)| *of == run)
        .map(|(_, seq)| *seq)
        .collect()
}

#[queue(name = "nestrs-e2e-replicas-fetch", job = SlowCommand)]
struct FetchQueue;

#[injectable]
#[derive(Default)]
struct FetchProcessor;

#[processor]
impl FetchProcessor {
    #[process(queue = FetchQueue, retries = 0)]
    async fn slow(&self, job: SlowCommand) -> anyhow::Result<()> {
        FETCH_RUNS.lock().expect("lock").push((job.run, job.seq));
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
/// already up, a batch is split between them and **no job is handed to both**. This is
/// the atomic claim in `get_jobs.lua`, measured rather than read — and, since
/// each replica logs the id it consumes under, the two ids are two.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_fetch_never_hands_one_job_to_two_replicas_and_each_consumes_as_itself() {
    const JOBS: usize = 4;
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    ghost_of_an_earlier_run("nestrs-e2e-replicas-fetch").await;

    // Both up before any job of this run exists, so no startup sweep can find
    // one of them in flight — this isolates the fetch from the sweep `lease`
    // covers. The ghost an earlier run left is swept, runs, and is not counted.
    let first = crate::replica::<FetchModule>().await;
    let second = crate::replica::<FetchModule>().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let conn = RedisQueueProducer::new(
        RedisConnection::connect(&crate::redis_config())
            .await
            .expect("connect"),
    );
    for seq in 0..JOBS {
        conn.push(FetchQueue, SlowCommand { seq, run }, None)
            .await
            .expect("enqueue");
    }

    // Serialized per replica, two replicas ⇒ ceil(JOBS / 2) waves, plus slack.
    crate::wait_until(
        HOLD * (JOBS as u32).div_ceil(2) + Duration::from_secs(5),
        || fetched_of(run).len() >= JOBS,
    )
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    first.worker.shutdown().await.expect("clean shutdown");
    second.worker.shutdown().await.expect("clean shutdown");

    let mut seen = fetched_of(run);
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
    // The interrupted attempt was a unit of work: it files its line, once, and
    // says it was stopped rather than that it failed.
    let lines = logs.find(
        nest_rs_core::operation_log::TARGET,
        nest_rs_queue::unit::JOB.name(),
    );
    let lines: Vec<_> = lines
        .iter()
        .filter(|line| crate::names(line, receipt.id()))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "one line for the interrupted attempt: {lines:#?}"
    );
    assert_eq!(
        lines[0].field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
    );
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
    assert_eq!(
        OUTLASTED.finished(run),
        1,
        "and completed by the next replica"
    );
}

// --- a shutdown while Redis stalls ----------------------------------------------

/// How long the stalled tests' proxy holds every answer back: past the brisk
/// drain's whole window, so the drain stops waiting before Redis answers.
const DRAIN_STALL: Duration = Duration::from_secs(4);

static CUT_STALLED: Runs = Runs::new();

const CUT_STALLED_QUEUE: &str = "nestrs-e2e-drain-stalled-cut";

#[queue(name = "nestrs-e2e-drain-stalled-cut", job = DrainCommand)]
struct CutStalledQueue;

#[injectable]
#[derive(Default)]
struct CutStalledProcessor;

#[processor]
impl CutStalledProcessor {
    /// No retry: an attempt left counted would be the job's last.
    #[process(queue = CutStalledQueue, retries = 0)]
    async fn hold(&self, job: DrainCommand) -> anyhow::Result<()> {
        hold(&CUT_STALLED, job).await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(None), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [CutStalledProcessor],
)]
struct CutStalledModule;

/// The drain window closes on a running attempt while Redis holds every answer
/// back past the reserve, so the drain stops waiting before any call of the
/// attempt's hand-back answers. The start the attempt never answered for is
/// given back all the same — sent first, the moment the window closes on it —
/// so the job is not one whose attempts never returned: with no retry left, the
/// next replica runs it again rather than dead-lettering it unrun. And the
/// drain's line says what it cut.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_attempt_cut_while_redis_stalls_gives_its_start_back_and_runs_again() {
    crate::forget(CUT_STALLED_QUEUE).await;
    let logs = LogCapture::install_global();
    let proxy = SlowProxy::start(DRAIN_STALL).await;
    let first = crate::replica_on::<CutStalledModule>(RedisConfig {
        url: proxy.url(),
        ..Default::default()
    })
    .await;
    let run = crate::this_run();
    let receipt = first
        .producer
        .push(
            CutStalledQueue,
            DrainCommand {
                run,
                hold_ms: 60_000,
            },
            None,
        )
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || CUT_STALLED.of(run).len() == 1).await;
    assert_eq!(CUT_STALLED.of(run).len(), 1, "the first attempt started");

    proxy.slow_down(true);
    first.worker.shutdown().await.expect("clean shutdown");
    let attempts = crate::key_of(CUT_STALLED_QUEUE, "attempts", &receipt.id().to_string());
    let counted = crate::read(&attempts).await;
    let cut = logs.find(
        nest_rs_queue::TARGET,
        "queue workers did not stop within the shutdown window; a delivery cut here leaves its \
         job in flight until a replica sweeps it, its attempt spent unless the give-back reached \
         Redis",
    );

    let second = crate::replica::<CutStalledModule>().await;
    crate::wait_until(Duration::from_secs(15), || CUT_STALLED.finished(run) == 1).await;
    second.worker.shutdown().await.expect("clean shutdown");
    crate::forget(CUT_STALLED_QUEUE).await;

    assert!(
        counted.as_deref().is_none_or(|count| count == "0"),
        "the cut attempt's start was given back, not left counted: {counted:?}",
    );
    assert_eq!(cut.len(), 1, "the drain said it stopped waiting: {cut:#?}");
    assert_eq!(
        cut[0].field("cut").as_deref(),
        Some("1"),
        "one delivery cut"
    );
    assert_eq!(
        CUT_STALLED.of(run).len(),
        2,
        "cut once, then run again by the next replica rather than dead-lettered unrun",
    );
    assert_eq!(CUT_STALLED.finished(run), 1, "and completed");
}

static SETTLED_STALLED: Runs = Runs::new();

const SETTLED_STALLED_QUEUE: &str = "nestrs-e2e-drain-stalled-settled";

#[queue(name = "nestrs-e2e-drain-stalled-settled", job = DrainCommand)]
struct SettledStalledQueue;

#[injectable]
#[derive(Default)]
struct SettledStalledProcessor;

#[processor]
impl SettledStalledProcessor {
    #[process(queue = SettledStalledQueue, retries = 0)]
    async fn hold(&self, job: DrainCommand) -> anyhow::Result<()> {
        hold(&SETTLED_STALLED, job).await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(None), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [SettledStalledProcessor],
)]
struct SettledStalledModule;

/// A replica that settled a job a moment ago stops while Redis holds every
/// answer back, longer than the connection's budget of ten seconds: the stop
/// stays inside its two-second window. Nothing it owes Redis is left to run after the drain — a settled
/// mark is written once, when its job settles, and no pass over the jobs
/// settled lately waits out a budget past the window the operator set.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_while_redis_stalls_after_a_settle_stays_inside_its_window() {
    crate::forget(SETTLED_STALLED_QUEUE).await;
    let config = RedisConfig::default();
    let proxy = SlowProxy::start(config.connect_timeout + DRAIN_STALL).await;
    let replica = crate::replica_on::<SettledStalledModule>(RedisConfig {
        url: proxy.url(),
        ..config
    })
    .await;
    let run = crate::this_run();
    replica
        .producer
        .push(SettledStalledQueue, DrainCommand { run, hold_ms: 0 }, None)
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || {
        SETTLED_STALLED.finished(run) == 1
    })
    .await;
    assert_eq!(SETTLED_STALLED.finished(run), 1, "the job ran and settled");

    proxy.slow_down(true);
    let stopping = Instant::now();
    replica.worker.shutdown().await.expect("clean shutdown");
    let took = stopping.elapsed();
    crate::forget(SETTLED_STALLED_QUEUE).await;

    let window = crate::brisk().shutdown_timeout;
    assert!(
        took < window + Duration::from_millis(500),
        "the stop kept inside its {window:?} window, not {took:?}",
    );
}

// --- due records reach the queue at the fetch's pace ---------------------------

/// How many records fall due at once: past a promotion's hundred, so reaching
/// `active` takes two of its scans — and a hundred and fifty of apalis's, at the
/// default concurrency's one record a scan.
const BURST: usize = 150;

/// Whether a job holding the only permit has started, and whether to let it go.
static BLOCKING: AtomicBool = AtomicBool::new(false);
static RELEASED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BurstCommand {
    blocks: bool,
}

const BURST_QUEUE: &str = "nestrs-e2e-promotion-burst";

#[queue(name = "nestrs-e2e-promotion-burst", job = BurstCommand)]
struct BurstQueue;

#[injectable]
#[derive(Default)]
struct BurstProcessor;

#[processor]
impl BurstProcessor {
    /// One permit: a job that blocks holds it, so the replica fetches nothing
    /// while the burst is moved onto its queue.
    #[process(queue = BurstQueue, retries = 0)]
    async fn run(&self, job: BurstCommand) -> anyhow::Result<()> {
        if job.blocks {
            BLOCKING.store(true, Ordering::SeqCst);
            while !RELEASED.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [BurstProcessor],
)]
struct BurstModule;

/// A burst of records filed due at once on the schedule — a peer's hand-backs,
/// filed through apalis's own `schedule_request` as a worker files one — is on
/// `active`, every record of it, within two of the worker's promotion scans,
/// while the method's one permit is held and nothing is fetched. Moved at the
/// fetch's pace by apalis's scan, the burst would have taken a hundred and fifty
/// seconds, one record a second.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_of_due_records_reaches_active_within_two_promotion_scans() {
    crate::forget(BURST_QUEUE).await;
    let replica = crate::replica::<BurstModule>().await;
    replica
        .producer
        .push(BurstQueue, BurstCommand { blocks: true }, None)
        .await
        .expect("enqueue the job that holds the permit");
    crate::wait_until(Duration::from_secs(15), || BLOCKING.load(Ordering::SeqCst)).await;
    assert!(
        BLOCKING.load(Ordering::SeqCst),
        "the permit is held before the burst"
    );

    let mut apalis: RedisStorage<serde_json::Value, nest_rs_redis::RedisConnection> =
        RedisStorage::new_with_config(
            crate::connect().await,
            Config::default().set_namespace(&crate::namespace(BURST_QUEUE)),
        );
    let due = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default();
    let mut filed = Vec::with_capacity(BURST);
    for _ in 0..BURST {
        let record = Request::new(serde_json::json!({
            "v": nest_rs_queue::WIRE_FORMAT_VERSION,
            "payload": { "blocks": false },
        }));
        let parts = apalis
            .schedule_request(record, due)
            .await
            .expect("a record filed due on the schedule");
        filed.push(parts.task_id.to_string());
    }
    let burst_filed = Instant::now();

    let mut admin = crate::connect().await;
    let active = format!("{}:active", crate::namespace(BURST_QUEUE));
    let mut waiting: Vec<String> = Vec::new();
    while burst_filed.elapsed() < Duration::from_secs(5) {
        waiting = redis::cmd("LRANGE")
            .arg(&active)
            .arg(0)
            .arg(-1)
            .query_async(&mut admin)
            .await
            .expect("LRANGE");
        if filed.iter().all(|id| waiting.contains(id)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let took = burst_filed.elapsed();

    RELEASED.store(true, Ordering::SeqCst);
    replica
        .worker
        .shutdown()
        .await
        .expect("clean worker shutdown");
    crate::forget(BURST_QUEUE).await;

    let promoted = filed.iter().filter(|id| waiting.contains(id)).count();
    assert_eq!(
        promoted, BURST,
        "every record of the burst reached active, not one fetch's worth a second"
    );
    assert!(
        took < Duration::from_secs(3),
        "within two promotion scans of a second each, not {took:?}"
    );
}

// --- a fetch Redis answers late --------------------------------------------------

/// A TCP proxy in front of the dev container Redis that, while it is slow, holds
/// every answer back for `delay` before passing it on — in order, the connection
/// up throughout: Redis paused by a failover, stalled by a fork or another
/// client's script, or a network that queues.
struct SlowProxy {
    addr: std::net::SocketAddr,
    slow: Arc<AtomicBool>,
}

impl SlowProxy {
    async fn start(delay: Duration) -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let upstream = crate::redis_address();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the proxy");
        let addr = listener.local_addr().expect("the proxy's address");
        let slow = Arc::new(AtomicBool::new(false));
        let slowing = Arc::clone(&slow);
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let Ok(server) = tokio::net::TcpStream::connect(&upstream).await else {
                    continue;
                };
                let (mut from_client, mut to_server) = (client.into_split(), server.into_split());
                tokio::spawn(async move {
                    let _ = tokio::io::copy(&mut from_client.0, &mut to_server.1).await;
                });
                let (answers, mut released) =
                    tokio::sync::mpsc::unbounded_channel::<(tokio::time::Instant, Vec<u8>)>();
                tokio::spawn(async move {
                    while let Some((due, chunk)) = released.recv().await {
                        tokio::time::sleep_until(due).await;
                        if from_client.1.write_all(&chunk).await.is_err() {
                            return;
                        }
                    }
                });
                let slowing = Arc::clone(&slowing);
                tokio::spawn(async move {
                    let mut chunk = vec![0u8; 64 * 1024];
                    loop {
                        let read = match to_server.0.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => read,
                        };
                        let held = if slowing.load(Ordering::SeqCst) {
                            delay
                        } else {
                            Duration::ZERO
                        };
                        let due = tokio::time::Instant::now() + held;
                        if answers.send((due, chunk[..read].to_vec())).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Self { addr, slow }
    }

    fn url(&self) -> String {
        format!("redis://{}/", self.addr)
    }

    fn slow_down(&self, slow: bool) {
        self.slow.store(slow, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StalledCommand {
    run: u64,
}

static STALLED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-fetch-stalled", job = StalledCommand)]
struct StalledQueue;

#[injectable]
#[derive(Default)]
struct StalledProcessor;

#[processor]
impl StalledProcessor {
    #[process(queue = StalledQueue, retries = 0)]
    async fn run(&self, job: StalledCommand) -> anyhow::Result<()> {
        STALLED.start(job.run);
        STALLED.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(None), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [StalledProcessor],
)]
struct StalledModule;

/// The budget the stalled replica's connection runs under, and how long its
/// answers are held back: three times as long.
const STALL_BUDGET: Duration = Duration::from_secs(1);
const STALL: Duration = Duration::from_secs(3);

/// How long a stalled queue has, once Redis answers at once again, to run what
/// the stall claimed and to answer every copy of it.
const SETTLE: Duration = Duration::from_secs(15);

/// What `queue` still holds for a worker, read in one script so no fetch or
/// sweep moves a job between the two counts: the jobs waiting on `active`, and
/// the jobs in flight across every consumer apalis knows.
async fn left_behind(conn: &mut RedisConnection, queue: &str) -> (i64, i64) {
    redis::Script::new(
        r"
local held = 0
for _, set in ipairs(redis.call('ZRANGE', KEYS[2], 0, -1)) do
  held = held + redis.call('SCARD', set)
end
return {redis.call('LLEN', KEYS[1]), held}
",
    )
    .key(format!("{}:active", crate::namespace(queue)))
    .key(format!("{}:consumers", crate::namespace(queue)))
    .invoke_async(conn)
    .await
    .expect("the queue's waiting and in-flight counts")
}

/// Redis answers a replica's fetch three budgets late. The fetch has already
/// claimed the job into the replica's flight when it runs, so a wait cut at the
/// budget would leave the job there — never run, never swept while the replica
/// lives, gone from the list an autoscaler reads. The fetch waits for its answer
/// instead, and the job runs, once, as soon as Redis answers again.
///
/// The same wait holds apalis's heartbeat, which its worker sends from the loop
/// the fetch runs in: a stall past the orphan threshold lets a sweep — a peer's,
/// or this replica's own — put the job it claimed back on the queue, beside the
/// delivery its fetch still makes. That copy is the guard's to answer, from the
/// settled mark, without running it. So the test waits for the queue to settle
/// rather than reading it at the instant the job ran: it passes only once the
/// job has run, nothing of the queue waits or sits in flight, and the job has
/// still run once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fetch_redis_answers_past_the_budget_still_runs_the_jobs_it_claimed() {
    let proxy = SlowProxy::start(STALL).await;
    let replica = crate::replica_on::<StalledModule>(RedisConfig {
        url: proxy.url(),
        connect_timeout: STALL_BUDGET,
        ..Default::default()
    })
    .await;
    let producer = crate::producer().await;

    let control = crate::this_run();
    producer
        .push(StalledQueue, StalledCommand { run: control }, None)
        .await
        .expect("enqueue the control job");
    crate::wait_until(Duration::from_secs(10), || STALLED.finished(control) == 1).await;
    assert_eq!(
        STALLED.finished(control),
        1,
        "the replica runs a job while Redis answers at once"
    );

    let run = crate::this_run();
    proxy.slow_down(true);
    producer
        .push(StalledQueue, StalledCommand { run }, None)
        .await
        .expect("enqueue straight to Redis, past the proxy");
    tokio::time::sleep(STALL + STALL_BUDGET).await;
    proxy.slow_down(false);

    let mut admin = crate::connect().await;
    let deadline = tokio::time::Instant::now() + SETTLE;
    let (waiting, in_flight) = loop {
        let left = left_behind(&mut admin, "nestrs-e2e-fetch-stalled").await;
        if (STALLED.finished(run) == 1 && left == (0, 0)) || tokio::time::Instant::now() > deadline
        {
            break left;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    replica.worker.shutdown().await.expect("clean shutdown");
    crate::forget("nestrs-e2e-fetch-stalled").await;

    assert_eq!(
        STALLED.finished(run),
        1,
        "the job the stalled fetch claimed ran, not left in the replica's flight",
    );
    assert_eq!(STALLED.of(run).len(), 1, "and ran once");
    assert_eq!(waiting, 0, "nothing waits on the queue behind it");
    assert_eq!(in_flight, 0, "and nothing is left in flight");
}

// --- a record apalis cannot decode ----------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PoisonCommand {
    run: u64,
}

static POISONED: Runs = Runs::new();

const POISON_QUEUE: &str = "nestrs-e2e-fetch-poison";

#[queue(name = "nestrs-e2e-fetch-poison", job = PoisonCommand)]
struct PoisonQueue;

#[injectable]
#[derive(Default)]
struct PoisonProcessor;

#[processor]
impl PoisonProcessor {
    #[process(queue = PoisonQueue, retries = 0, concurrency = 4)]
    async fn run(&self, job: PoisonCommand) -> anyhow::Result<()> {
        POISONED.start(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [PoisonProcessor],
)]
struct PoisonModule;

/// A record on the queue that is not one apalis can decode — written by hand,
/// by a foreign producer, by a version apalis no longer reads — fails the whole
/// fetch that claimed it, after the claim: apalis-redis 0.7.4 decodes a batch
/// once it is in flight, and drops it at the first record it cannot read. The
/// job fetched beside it then waits in the replica's flight, and nothing in the
/// worker can route it through the port's refusal, since apalis owns the fetch.
/// What the worker owes is the line saying so, at `error`, naming the queue —
/// never the generic retry a transport failure gets.
#[expect(
    clippy::disallowed_methods,
    reason = "the suite plays what apalis files, where apalis files it"
)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fetch_meeting_a_record_apalis_cannot_decode_says_what_it_stranded() {
    crate::forget(POISON_QUEUE).await;
    let logs = LogCapture::install_global();
    let producer = crate::producer().await;
    let run = crate::this_run();
    producer
        .push(PoisonQueue, PoisonCommand { run }, None)
        .await
        .expect("enqueue the job fetched beside the record");
    let apalis = Config::default().set_namespace(&crate::namespace(POISON_QUEUE));
    let mut admin = crate::connect().await;
    let _: () = redis::pipe()
        .hset(apalis.job_data_hash(), "poison", "this is not json")
        .ignore()
        .lpush(apalis.active_jobs_list(), "poison")
        .ignore()
        .query_async(&mut admin)
        .await
        .expect("file a record apalis cannot decode");

    let replica = crate::replica::<PoisonModule>().await;
    let said = "queue fetch met a record apalis cannot decode; it and every job fetched beside \
                it wait in flight until a replica starts";
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && logs.find(nest_rs_queue::TARGET, said).is_empty() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    replica.worker.shutdown().await.expect("clean shutdown");
    crate::forget(POISON_QUEUE).await;

    let stranded: Vec<_> = logs
        .find(nest_rs_queue::TARGET, said)
        .into_iter()
        .filter(|event| event.field("queue").as_deref() == Some(POISON_QUEUE))
        .collect();
    assert_eq!(
        stranded.len(),
        1,
        "said once, for the one fetch: {stranded:#?}"
    );
    assert_eq!(stranded[0].level, "error");
    assert!(
        logs.find(
            nest_rs_queue::TARGET,
            "queue fetch failed; retrying at the next poll, and any job it claimed before failing \
             waits in flight until a replica starts",
        )
        .iter()
        .all(|event| event.field("queue").as_deref() != Some(POISON_QUEUE)),
        "and not as a transport failure",
    );
    assert!(
        POISONED.of(run).is_empty(),
        "the job fetched beside it waited, which is apalis's limit this line names",
    );
}
