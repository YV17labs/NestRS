//! The Redis queue's consumer under the port's worker, against a live Redis:
//! the behaviour kit every queue backend runs, then what only Redis decides —
//! the records a job leaves when it ends, its dead letter, a checkpoint through
//! a retry and a dead replica, a throttle across replicas, a cancel refused
//! while a delivery runs, a group made again after its stream went, a read
//! blocking on a connection of its own, and a stopped worker leaving its group.

use std::sync::Mutex;
use std::time::Duration;

use nest_rs_core::{injectable, module};
use nest_rs_queue::{Checkpoint, JobProducerExt, QueueName, processor, queue};
use nest_rs_redis::{RedisConnection, RedisModule, RedisQueueConfig, RedisQueueModule};
use nest_rs_testing::TestApp;
use nest_rs_testing::queue::KitBackend;
use serde::{Deserialize, Serialize};

use crate::Runs;

/// The consumer group every worker reads a queue's stream through.
const GROUP: &str = "workers";

#[module(imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule])]
struct KitModule;

/// The kit's backend: the shared Redis, each case on a queue of its own.
struct RedisKit;

impl RedisKit {
    /// Every delivery pending on `queue`'s stream, and who holds it.
    async fn pending(queue: &QueueName) -> anyhow::Result<Vec<(String, String)>> {
        let lines: Vec<(String, String, u64, u64)> = redis::cmd("XPENDING")
            .arg(crate::key_of(queue.as_str(), "jobs"))
            .arg(GROUP)
            .arg("-")
            .arg("+")
            .arg(1000)
            .query_async(&mut crate::connect().await)
            .await?;
        Ok(lines
            .into_iter()
            .map(|(entry, holder, _, _)| (entry, holder))
            .collect())
    }

    /// Claim each delivery of `queue` for `holder` (its own holder when
    /// `None`) as idle past any lease — counting a delivery when it changes
    /// hands.
    async fn idle(queue: &QueueName, holder: Option<&str>) -> anyhow::Result<()> {
        let idle = u64::try_from(crate::LEASE.as_millis() * 10)?;
        for (entry, held_by) in Self::pending(queue).await? {
            let mut claim = redis::cmd("XCLAIM");
            claim
                .arg(crate::key_of(queue.as_str(), "jobs"))
                .arg(GROUP)
                .arg(holder.unwrap_or(&held_by))
                .arg(0)
                .arg(&entry)
                .arg("IDLE")
                .arg(idle);
            if holder.is_none() {
                claim.arg("JUSTID");
            }
            let _: redis::Value = claim.query_async(&mut crate::connect().await).await?;
        }
        Ok(())
    }
}

impl KitBackend for RedisKit {
    fn app(&self) -> nest_rs_testing::TestAppBuilder {
        TestApp::builder()
            .provide(RedisQueueConfig {
                lease: crate::LEASE,
            })
            .module::<KitModule>()
    }

    fn lease(&self) -> Duration {
        crate::LEASE
    }

    async fn purge(&self, queue: &QueueName) -> anyhow::Result<()> {
        crate::forget(queue.as_str()).await;
        Ok(())
    }

    async fn lapse(&self, queue: &QueueName) -> anyhow::Result<()> {
        Self::idle(queue, None).await
    }

    async fn take(&self, queue: &QueueName) -> anyhow::Result<()> {
        Self::idle(queue, Some("nestrs-e2e-kit-ghost")).await
    }
}

/// The kit, against Redis.
mod kit {
    use super::RedisKit;

    nest_rs_testing::queue_kit!(RedisKit);
}

// --- what a job leaves ---------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EndCommand {
    run: u64,
    fail: bool,
}

static ENDED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-consumer-end", job = EndCommand)]
struct EndQueue;

#[injectable]
#[derive(Default)]
struct EndProcessor;

#[processor]
impl EndProcessor {
    #[process(queue = EndQueue, retries = 0, transactional = false)]
    async fn end(&self, job: EndCommand, mut progress: Checkpoint<u32>) -> anyhow::Result<()> {
        ENDED.start(job.run);
        progress.save(1).await?;
        if job.fail {
            anyhow::bail!("the upstream refused the job");
        }
        ENDED.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, nest_rs_queue::QueueModule::for_root(None)],
    providers = [EndProcessor],
)]
struct EndModule;

/// A job that completed and one that dead-lettered — each under a unique key,
/// each with a checkpoint — leave nothing behind but the stream they ran from,
/// empty, and the one dead letter: its job, its record as stored, and the
/// reason its line rendered, which quotes no payload value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_that_ended_leaves_no_record_but_its_dead_letter() {
    let queue = <EndQueue as nest_rs_queue::Queue>::NAME;
    crate::forget(queue).await;
    let run = crate::this_run();
    let replica = crate::replica::<EndModule>().await;
    let unique = |key: &str| nest_rs_queue::PushOptions::default().with_unique(key);
    replica
        .producer
        .push(EndQueue, EndCommand { run, fail: false }, unique("done"))
        .await
        .expect("a push that completes");
    let doomed = replica
        .producer
        .push(
            EndQueue,
            EndCommand {
                run: run + 1,
                fail: true,
            },
            unique("doomed"),
        )
        .await
        .expect("a push that dead-letters");

    let dead_letters = || async {
        redis::cmd("XLEN")
            .arg(crate::key_of(queue, "dead"))
            .query_async::<i64>(&mut crate::connect().await)
            .await
            .expect("XLEN")
    };
    crate::wait_for(Duration::from_secs(10), || async move {
        ENDED.finished(run) == 1 && dead_letters().await == 1
    })
    .await;
    replica.worker.shutdown().await.expect("clean shutdown");

    assert_eq!(ENDED.finished(run), 1, "the first job completed");
    assert_eq!(
        crate::keys_of(queue).await,
        [crate::key_of(queue, "dead"), crate::key_of(queue, "jobs")],
        "no record of either job is left but the dead letter",
    );
    assert_eq!(crate::filed(queue).await, 0, "and the stream is empty");
    let dead: Vec<(String, Vec<(String, String)>)> = redis::cmd("XRANGE")
        .arg(crate::key_of(queue, "dead"))
        .arg("-")
        .arg("+")
        .query_async(&mut crate::connect().await)
        .await
        .expect("XRANGE");
    let fields = &dead[0].1;
    let field = |name: &str| {
        fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| panic!("a dead letter carries `{name}`: {fields:?}"))
    };
    assert_eq!(field("job"), doomed.id().to_string());
    let record: serde_json::Value = serde_json::from_str(&field("record")).expect("JSON");
    assert_eq!(record["payload"]["run"], run + 1, "the record as stored");
    let reason = field("reason");
    assert!(reason.contains("the upstream refused the job"), "{reason}");
    assert_eq!(
        field("unique"),
        "doomed",
        "the unique key it held, to take again"
    );
    assert!(
        !reason.contains(&(run + 1).to_string()),
        "no payload value: {reason}"
    );
    crate::forget(queue).await;
}

// --- checkpoints ---------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ImportCommand {
    run: u64,
}

/// What each attempt at a run's job read from its checkpoint, in order.
static SEEN: Mutex<Vec<(u64, Option<u32>)>> = Mutex::new(Vec::new());

static IMPORTED: Runs = Runs::new();

fn seen(run: u64) -> Vec<Option<u32>> {
    SEEN.lock()
        .expect("seen lock")
        .iter()
        .filter(|(of, _)| *of == run)
        .map(|(_, progress)| *progress)
        .collect()
}

#[queue(name = "nestrs-e2e-checkpoint", job = ImportCommand)]
struct ImportQueue;

#[injectable]
#[derive(Default)]
struct ImportProcessor;

#[processor]
impl ImportProcessor {
    /// The first attempt saves `1` and fails retryably; the second saves `2` and
    /// holds on until its replica dies; the delivery after it resumes and
    /// completes.
    #[process(queue = ImportQueue, retries = 1, transactional = false)]
    async fn import(
        &self,
        job: ImportCommand,
        mut progress: Checkpoint<u32>,
    ) -> anyhow::Result<()> {
        IMPORTED.start(job.run);
        let read = progress.get().copied();
        SEEN.lock().expect("seen lock").push((job.run, read));
        match read {
            None => {
                progress.save(1).await?;
                anyhow::bail!("the upstream timed out");
            }
            Some(1) => {
                progress.save(2).await?;
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
            Some(_) => {}
        }
        IMPORTED.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, nest_rs_queue::QueueModule::for_root(None)],
    providers = [ImportProcessor],
)]
struct ImportModule;

/// A save outlives the attempt that failed after it and the replica that died
/// holding it — whose death costs a stall, not an attempt — and is cleared when
/// the job completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_checkpoint_survives_a_retry_and_a_dead_replica_and_is_cleared_at_the_outcome() {
    let queue = <ImportQueue as nest_rs_queue::Queue>::NAME;
    crate::forget(queue).await;
    let run = crate::this_run();
    let doomed = crate::mortal_replica::<ImportModule>().await;
    let receipt = doomed
        .producer
        .push(ImportQueue, ImportCommand { run }, None)
        .await
        .expect("enqueue");
    let job = receipt.id().to_string();

    // The second attempt read the first one's save, and saved its own.
    crate::wait_until(Duration::from_secs(15), || seen(run).len() == 2).await;
    let job = job.as_str();
    // The second attempt's save is in Redis.
    crate::wait_for(Duration::from_secs(2), || async move {
        crate::field_of(queue, "checkpoints", job).await.as_deref() == Some("2")
    })
    .await;

    doomed.kill().await;
    let heir = crate::replica::<ImportModule>().await;
    crate::wait_until(Duration::from_secs(20), || IMPORTED.finished(run) == 1).await;
    // Its checkpoint went with it.
    crate::wait_for(Duration::from_secs(2), || async move {
        crate::field_of(queue, "checkpoints", job).await.is_none()
    })
    .await;
    heir.worker.shutdown().await.expect("clean shutdown");

    assert_eq!(
        seen(run),
        [None, Some(1), Some(2)],
        "each attempt resumed from the last save: after a retry, and after a dead replica",
    );
    assert_eq!(IMPORTED.finished(run), 1, "the job completed once");
    crate::forget(queue).await;
}

// --- a throttle across replicas ------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PacedCommand {
    run: u64,
}

static PACED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-throttle", job = PacedCommand)]
struct PacedQueue;

#[injectable]
#[derive(Default)]
struct PacedProcessor;

#[processor]
impl PacedProcessor {
    #[process(queue = PacedQueue, concurrency = 4, throttle(limit = 2, window = "1s"))]
    async fn pace(&self, job: PacedCommand) -> anyhow::Result<()> {
        PACED.start(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, nest_rs_queue::QueueModule::for_root(None)],
    providers = [PacedProcessor],
)]
struct PacedModule;

/// Two replicas share one window: however many permits each has free, no more
/// than the limit start inside any window, and the rest wait on the queue for
/// the next.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_throttle_caps_starts_per_window_across_replicas() {
    let queue = <PacedQueue as nest_rs_queue::Queue>::NAME;
    crate::forget(queue).await;
    let run = crate::this_run();
    let first = crate::replica::<PacedModule>().await;
    let second = crate::replica::<PacedModule>().await;
    for _ in 0..6 {
        first
            .producer
            .push(PacedQueue, PacedCommand { run }, None)
            .await
            .expect("enqueue");
    }
    crate::wait_until(Duration::from_secs(10), || PACED.of(run).len() == 6).await;
    first.worker.shutdown().await.expect("clean shutdown");
    second.worker.shutdown().await.expect("clean shutdown");

    let starts = PACED.of(run);
    assert_eq!(starts.len(), 6, "every job ran");
    let first_start = starts.iter().min().copied().expect("a start");
    let span = starts.iter().max().copied().expect("a start") - first_start;
    // Two windows hold four starts at most, so six take a third, which opens
    // more than a window after the first start — however late in its window
    // the first started.
    assert!(
        span > Duration::from_secs(1),
        "six starts at two a window take three windows: {span:?}",
    );
    for (at, start) in starts.iter().enumerate() {
        let within = starts
            .iter()
            .filter(|other| {
                **other >= *start && other.duration_since(*start) < Duration::from_millis(900)
            })
            .count();
        assert!(
            within <= 4,
            "start {at}: {within} started within a window of it"
        );
    }
    crate::forget(queue).await;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BriskCommand {
    run: u64,
}

static BRISK: Runs = Runs::new();

#[queue(name = "nestrs-e2e-throttle-edge", job = BriskCommand)]
struct BriskQueue;

#[injectable]
#[derive(Default)]
struct BriskProcessor;

#[processor]
impl BriskProcessor {
    #[process(queue = BriskQueue, throttle(limit = 1000, window = "2ms"), transactional = false)]
    async fn brisk(&self, job: BriskCommand) -> anyhow::Result<()> {
        BRISK.start(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, nest_rs_queue::QueueModule::for_root(None)],
    providers = [BriskProcessor],
)]
struct BriskModule;

/// A receive counted into a window in its last millisecond, with room left in
/// it, starts its job rather than failing: Redis answers that window's time to
/// live as `0`, which no expiry may be. Every receive here asks for one start of
/// a thousand, so a window never fills and each one ends with room left.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_throttle_window_in_its_last_millisecond_opens_the_next() {
    let logs = nest_rs_testing::LogCapture::install_global();
    let queue = <BriskQueue as nest_rs_queue::Queue>::NAME;
    crate::forget(queue).await;
    let run = crate::this_run();
    let replica = crate::replica::<BriskModule>().await;
    let jobs = 300;
    replica
        .producer
        .push_many(BriskQueue, (0..jobs).map(|_| BriskCommand { run }), None)
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(20), || BRISK.of(run).len() == jobs).await;
    replica.worker.shutdown().await.expect("clean shutdown");

    let failed: Vec<_> = logs
        .find(nest_rs_queue::TARGET, "queue receive failed; retrying")
        .into_iter()
        .filter_map(|line| line.field("error"))
        .collect();
    assert!(
        failed.is_empty(),
        "{} receives failed, the first: {:?}",
        failed.len(),
        failed.first(),
    );
    assert_eq!(BRISK.of(run).len(), jobs, "every job ran");
    crate::forget(queue).await;
}

// --- a cancel while a delivery runs --------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HeldCommand {
    run: u64,
}

static HELD: Runs = Runs::new();

/// Opened by the test once the cancel has answered, so the job is running when
/// it is asked.
static HOLD: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(0);

#[queue(name = "nestrs-e2e-cancel-running", job = HeldCommand)]
struct HeldQueue;

#[injectable]
#[derive(Default)]
struct HeldProcessor;

#[processor]
impl HeldProcessor {
    #[process(queue = HeldQueue)]
    async fn hold(&self, job: HeldCommand) -> anyhow::Result<()> {
        HELD.start(job.run);
        HOLD.acquire().await?.forget();
        HELD.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, nest_rs_queue::QueueModule::for_root(None)],
    providers = [HeldProcessor],
)]
struct HeldModule;

/// A cancel while a delivery runs the job answers `false` and touches nothing:
/// the job runs to its end, once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_while_an_attempt_runs_is_refused_and_the_job_runs_to_its_end() {
    let queue = <HeldQueue as nest_rs_queue::Queue>::NAME;
    crate::forget(queue).await;
    let run = crate::this_run();
    let replica = crate::replica::<HeldModule>().await;
    let receipt = replica
        .producer
        .push(HeldQueue, HeldCommand { run }, None)
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || HELD.of(run).len() == 1).await;
    assert!(
        !replica.producer.cancel(&receipt).await.expect("a cancel"),
        "a running job is not cancelled",
    );
    HOLD.add_permits(1);
    crate::wait_until(Duration::from_secs(10), || HELD.finished(run) == 1).await;
    replica.worker.shutdown().await.expect("clean shutdown");
    assert_eq!(HELD.of(run).len(), 1, "it ran once");
    assert_eq!(HELD.finished(run), 1, "to its end");
    crate::forget(queue).await;
}

// --- the group, the read, the leave --------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlainCommand {
    run: u64,
}

static PLAIN: Runs = Runs::new();

#[queue(name = "nestrs-e2e-consumer-group", job = PlainCommand)]
struct PlainQueue;

#[injectable]
#[derive(Default)]
struct PlainProcessor;

#[processor]
impl PlainProcessor {
    #[process(queue = PlainQueue)]
    async fn run(&self, job: PlainCommand) -> anyhow::Result<()> {
        PLAIN.start(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, nest_rs_queue::QueueModule::for_root(None)],
    providers = [PlainProcessor],
)]
struct PlainModule;

/// The consumers of `queue`'s group, on `admin`'s database.
async fn consumers(admin: &mut RedisConnection, queue: &str) -> Vec<String> {
    let consumers: Vec<std::collections::HashMap<String, redis::Value>> = redis::cmd("XINFO")
        .arg("CONSUMERS")
        .arg(crate::key_of(queue, "jobs"))
        .arg(GROUP)
        .query_async(admin)
        .await
        .unwrap_or_default();
    consumers
        .iter()
        .filter_map(|consumer| {
            consumer
                .get("name")
                .and_then(|name| redis::from_redis_value_ref::<String>(name).ok())
        })
        .collect()
}

/// Empty `admin`'s database: what a flush does to a running worker, and what
/// a test owning the database leaves behind.
async fn flush(admin: &mut RedisConnection) {
    redis::cmd("FLUSHDB")
        .query_async::<()>(admin)
        .await
        .expect("FLUSHDB");
}

/// A stream deleted under a running worker — by hand, or a flush — takes its
/// group with it; the worker makes it again and runs what is filed next. An
/// idle worker's read blocks on a connection of its own, so a push answers at
/// once meanwhile; and a worker that stops leaves the group. On a database of
/// its own, the one its blocked read is found on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_group_is_made_again_after_its_stream_went_and_a_stopped_worker_leaves_it() {
    let queue = <PlainQueue as nest_rs_queue::Queue>::NAME;
    let mut admin = crate::connect_on(crate::DB_BLOCKED_READ).await;
    flush(&mut admin).await;
    assert_eq!(
        crate::blocked_reads_on(crate::DB_BLOCKED_READ).await,
        0,
        "no read blocks on the test's database before its worker starts",
    );
    let run = crate::this_run();
    let replica =
        crate::replica_on::<PlainModule>(crate::redis_config_on(crate::DB_BLOCKED_READ)).await;
    replica
        .producer
        .push(PlainQueue, PlainCommand { run }, None)
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || PLAIN.of(run).len() == 1).await;
    assert_eq!(
        consumers(&mut admin, queue).await.len(),
        1,
        "the worker reads as one consumer"
    );

    assert!(
        crate::a_read_blocks_on(crate::DB_BLOCKED_READ).await,
        "an idle worker's read blocks on a connection of its own",
    );
    let started = std::time::Instant::now();
    replica
        .producer
        .push(PlainQueue, PlainCommand { run: run + 1 }, None)
        .await
        .expect("a push while the read blocks");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "a push does not wait behind the read: {:?}",
        started.elapsed(),
    );
    crate::wait_until(Duration::from_secs(10), || PLAIN.of(run + 1).len() == 1).await;

    flush(&mut admin).await;
    replica
        .producer
        .push(PlainQueue, PlainCommand { run: run + 2 }, None)
        .await
        .expect("a push after the stream went");
    crate::wait_until(Duration::from_secs(10), || PLAIN.of(run + 2).len() == 1).await;
    assert_eq!(
        PLAIN.of(run + 2).len(),
        1,
        "the job filed after the stream went ran"
    );

    replica.worker.shutdown().await.expect("clean shutdown");
    assert!(
        consumers(&mut admin, queue).await.is_empty(),
        "a worker that stopped holding nothing leaves the group",
    );
    flush(&mut admin).await;
}

// --- an acknowledgement lost on its way back -----------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GatedCommand {
    run: u64,
}

static GATED: Runs = Runs::new();

/// Opened by the test once Redis's replies are being dropped.
static GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(0);

#[queue(name = "nestrs-e2e-lost-ack", job = GatedCommand)]
struct GatedQueue;

#[injectable]
#[derive(Default)]
struct GatedProcessor;

#[processor]
impl GatedProcessor {
    #[process(queue = GatedQueue)]
    async fn gated(&self, job: GatedCommand) -> anyhow::Result<()> {
        GATED.start(job.run);
        GATE.acquire().await?.forget();
        GATED.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(None), RedisQueueModule, nest_rs_queue::QueueModule::for_root(None)],
    providers = [GatedProcessor],
)]
struct GatedModule;

// --- a dead letter filed back --------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RefiledCommand {
    run: u64,
}

static REFILED: Runs = Runs::new();

/// Whether the job's cause is fixed: its attempts fail until then.
static FIXED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[queue(name = "nestrs-e2e-refile", job = RefiledCommand)]
struct RefiledQueue;

#[injectable]
#[derive(Default)]
struct RefiledProcessor;

#[processor]
impl RefiledProcessor {
    #[process(queue = RefiledQueue, retries = 0)]
    async fn refiled(&self, job: RefiledCommand) -> anyhow::Result<()> {
        REFILED.start(job.run);
        anyhow::ensure!(
            FIXED.load(std::sync::atomic::Ordering::SeqCst),
            "the upstream is down"
        );
        REFILED.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, nest_rs_queue::QueueModule::for_root(None)],
    providers = [RefiledProcessor],
)]
struct RefiledModule;

/// The command the delivery page prints to file a dead letter back, read off
/// the page as an operator copies it: its script, and its keys — printed for
/// the queue `audio` — for `queue`.
fn documented_refile(queue: &str) -> (String, Vec<String>) {
    let page = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/src/content/docs/queue/delivery.mdx");
    let text = std::fs::read_to_string(&page).expect("the delivery page reads");
    let line = text
        .lines()
        .find(|line| line.starts_with("valkey-cli EVAL \""))
        .expect("the delivery page prints the command that files a dead letter back");
    let quoted = &line["valkey-cli EVAL \"".len()..];
    let end = quoted.find('"').expect("the script is quoted");
    let mut rest = quoted[end + 1..].split_whitespace();
    let count: usize = rest
        .next()
        .and_then(|count| count.parse().ok())
        .expect("the command gives its number of keys");
    let keys = rest
        .take(count)
        .map(|key| {
            key.trim_matches('\'')
                .replace("{audio}", &format!("{{{queue}}}"))
        })
        .collect();
    (quoted[..end].to_owned(), keys)
}

/// File the dead letter `entry` of `queue` back with the page's command, over
/// `conn`.
async fn refile(
    conn: &mut nest_rs_redis::RedisConnection,
    queue: &str,
    entry: &str,
) -> redis::RedisResult<Option<String>> {
    let (script, keys) = documented_refile(queue);
    redis::cmd("EVAL")
        .arg(script)
        .arg(keys.len())
        .arg(keys)
        .arg(entry)
        .query_async(conn)
        .await
}

/// A dead letter filed back with the page's command runs once more, as the
/// attempt it died on, and leaves the dead letters.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dead_letter_filed_back_as_the_page_prints_runs_once_more() {
    let queue = <RefiledQueue as nest_rs_queue::Queue>::NAME;
    crate::forget(queue).await;
    let run = crate::this_run();
    let replica = crate::replica::<RefiledModule>().await;
    replica
        .producer
        .push(RefiledQueue, RefiledCommand { run }, None)
        .await
        .expect("enqueue");
    let dead = || async {
        let entries: Vec<(String, Vec<(String, String)>)> = redis::cmd("XRANGE")
            .arg(crate::key_of(queue, "dead"))
            .arg("-")
            .arg("+")
            .query_async(&mut crate::connect().await)
            .await
            .expect("XRANGE");
        entries
    };
    crate::wait_for(Duration::from_secs(10), || async {
        !dead().await.is_empty()
    })
    .await;
    let letters = dead().await;
    assert_eq!(letters.len(), 1, "the job dead-lettered once");

    FIXED.store(true, std::sync::atomic::Ordering::SeqCst);
    let refiled = refile(&mut crate::connect().await, queue, &letters[0].0)
        .await
        .expect("the page's command runs");
    assert!(refiled.is_some(), "it filed the dead letter back");
    crate::wait_until(Duration::from_secs(10), || REFILED.finished(run) == 1).await;
    replica.worker.shutdown().await.expect("clean shutdown");

    assert_eq!(REFILED.of(run).len(), 2, "it ran once more");
    assert_eq!(REFILED.finished(run), 1, "and completed");
    assert!(dead().await.is_empty(), "the dead letter is gone");
    assert_eq!(crate::filed(queue).await, 0);
    crate::forget(queue).await;
}

/// The ACL user the refile test creates from the page, and removes.
const OPERATOR_USER: &str = "nestrs-e2e-queue-operator";

/// The page's command, run as a user created from the page's operator rule, files
/// a dead letter back under the unique key its job held — taking it again — and
/// refuses one whose key another job took meanwhile, naming that job and filing
/// nothing, since two jobs under one key would break the key's promise. Redis
/// denies the user nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dead_letter_filed_back_takes_its_unique_key_again_or_names_its_holder() {
    let queue = format!("nestrs-e2e-refile-unique-{}", crate::this_run());
    let user = crate::acl_user(OPERATOR_USER);
    let config = crate::documented_user("queue/delivery.mdx", "operator", &user, 0).await;
    let mut operator = nest_rs_redis::RedisConnection::connect(&config)
        .await
        .expect("the page's user passes the boot's proof");
    let dead = crate::key_of(&queue, "dead");
    let letter = |job: &'static str, key: &'static str| {
        let dead = dead.clone();
        async move {
            redis::cmd("XADD")
                .arg(dead)
                .arg("*")
                .arg("job")
                .arg(job)
                .arg("record")
                .arg("{}")
                .arg("reason")
                .arg("the upstream is down")
                .arg("unique")
                .arg(key)
                .query_async::<String>(&mut crate::connect().await)
                .await
                .expect("a dead letter under a unique key")
        }
    };
    let freed = "01890a5d-ac96-774b-bcce-b302099a8057";
    let taken = "01890a5d-ac96-774b-bcce-b302099a8058";
    let holder = "01890a5d-ac96-774b-bcce-b302099a8059";

    let entry = letter(freed, "invoice-7").await;
    let filed = refile(&mut operator, &queue, &entry)
        .await
        .expect("a dead letter whose key is free is filed back");
    assert_eq!(
        crate::field_of(&queue, "unique", "invoice-7")
            .await
            .as_deref(),
        Some(freed),
        "its job holds its key again",
    );
    assert_eq!(
        crate::field_of(&queue, "claims", freed).await.as_deref(),
        Some("invoice-7")
    );
    assert_eq!(crate::field_of(&queue, "entries", freed).await, filed);

    let _: i64 = redis::cmd("HSET")
        .arg(crate::key_of(&queue, "unique"))
        .arg("invoice-8")
        .arg(holder)
        .query_async(&mut crate::connect().await)
        .await
        .expect("another job took the key");
    let entry = letter(taken, "invoice-8").await;
    let refused = refile(&mut operator, &queue, &entry)
        .await
        .expect_err("a dead letter whose key another job holds is refused");
    crate::forget_user(&user).await;
    assert!(refused.to_string().contains(holder), "{refused}");
    assert_eq!(crate::field_of(&queue, "entries", taken).await, None);
    assert_eq!(crate::filed(&queue).await, 1, "nothing more was filed");
    let kept: i64 = redis::cmd("XLEN")
        .arg(&dead)
        .query_async(&mut crate::connect().await)
        .await
        .expect("XLEN");
    assert_eq!(kept, 1, "the refused dead letter stays");
    crate::assert_redis_denied_nothing_but(&user, &[]).await;
    crate::forget(&queue).await;
}

// --- what the upkeep keeps tidy ------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrimCommand {
    run: u64,
}

#[queue(name = "nestrs-e2e-dead-trim", job = TrimCommand)]
struct TrimQueue;

#[injectable]
#[derive(Default)]
struct TrimProcessor;

#[processor]
impl TrimProcessor {
    #[process(queue = TrimQueue)]
    async fn trim(&self, _job: TrimCommand) -> anyhow::Result<()> {
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, nest_rs_queue::QueueModule::for_root(None)],
    providers = [TrimProcessor],
)]
struct TrimModule;

/// A dead letter older than a week goes at the next upkeep, whether or not
/// another job dies: the stream is trimmed by age as the queue runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dead_letter_past_its_week_is_trimmed_by_the_upkeep() {
    let queue = <TrimQueue as nest_rs_queue::Queue>::NAME;
    crate::forget(queue).await;
    let eight_days_ago = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .saturating_sub(Duration::from_secs(8 * 24 * 60 * 60))
        .as_millis();
    let _: String = redis::cmd("XADD")
        .arg(crate::key_of(queue, "dead"))
        .arg(format!("{eight_days_ago}-0"))
        .arg("job")
        .arg("01890a5d-ac96-774b-bcce-b302099a8057")
        .arg("record")
        .arg("{}")
        .arg("reason")
        .arg("long ago")
        .query_async(&mut crate::connect().await)
        .await
        .expect("an old dead letter");
    let replica = crate::replica::<TrimModule>().await;
    let dead = || async {
        redis::cmd("XLEN")
            .arg(crate::key_of(queue, "dead"))
            .query_async::<i64>(&mut crate::connect().await)
            .await
            .expect("XLEN")
    };
    // The week-old dead letter went.
    crate::wait_for(Duration::from_secs(3), || async { dead().await == 0 }).await;
    replica.worker.shutdown().await.expect("clean shutdown");
    crate::forget(queue).await;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LostCommand {
    run: u64,
}

static LOST: Runs = Runs::new();

#[queue(name = "nestrs-e2e-delayed-lost", job = LostCommand)]
struct LostQueue;

#[injectable]
#[derive(Default)]
struct LostProcessor;

#[processor]
impl LostProcessor {
    #[process(queue = LostQueue)]
    async fn lost(&self, job: LostCommand) -> anyhow::Result<()> {
        LOST.start(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, nest_rs_queue::QueueModule::for_root(None)],
    providers = [LostProcessor],
)]
struct LostModule;

/// A held-back job whose record was deleted from `…:delayed` — by hand, since no
/// script removes one a job still waits on — is gone with it: the upkeep that
/// finds it due says so, naming the job, and lets go of what it held, so a later
/// push under its unique key is filed and runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_back_job_whose_record_went_is_said_and_let_go() {
    let logs = nest_rs_testing::LogCapture::install_global();
    let queue = <LostQueue as nest_rs_queue::Queue>::NAME;
    crate::forget(queue).await;
    let run = crate::this_run();
    let replica = crate::replica::<LostModule>().await;
    let unique = || nest_rs_queue::PushOptions::default().with_unique("lost");
    let held = replica
        .producer
        .push(
            LostQueue,
            LostCommand { run },
            unique().with_delay(Duration::from_millis(300)),
        )
        .await
        .expect("a held-back push");
    let job = held.id().to_string();
    let _: i64 = redis::cmd("HDEL")
        .arg(crate::key_of(queue, "delayed"))
        .arg(&job)
        .query_async(&mut crate::connect().await)
        .await
        .expect("its record deleted by hand");
    let said = "held-back queue jobs found due without their record are gone; what they held \
                is let go";
    crate::wait_until(Duration::from_secs(4), || {
        !logs.find(nest_rs_queue::TARGET, said).is_empty()
    })
    .await;
    let line = logs.expect_one(nest_rs_queue::TARGET, said);
    assert_eq!(line.level, "warn");
    assert_eq!(line.field("job_ids").as_deref(), Some(job.as_str()));
    assert_eq!(crate::field_of(queue, "unique", "lost").await, None);

    replica
        .producer
        .push(LostQueue, LostCommand { run: run + 1 }, unique())
        .await
        .expect("a later push under the key it held");
    crate::wait_until(Duration::from_secs(10), || LOST.of(run + 1).len() == 1).await;
    replica.worker.shutdown().await.expect("clean shutdown");
    assert!(
        LOST.of(run).is_empty(),
        "the job whose record went never ran"
    );
    assert_eq!(LOST.of(run + 1).len(), 1, "the later push ran");
    assert_eq!(
        crate::keys_of(queue).await,
        [crate::key_of(queue, "jobs")],
        "nothing of either job is left",
    );
    crate::forget(queue).await;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LongCommand {
    run: u64,
}

static LONG: Runs = Runs::new();

#[queue(name = "nestrs-e2e-vanished", job = LongCommand)]
struct LongQueue;

#[injectable]
#[derive(Default)]
struct LongProcessor;

#[processor]
impl LongProcessor {
    /// Long enough for several renewals, however loaded the machine.
    #[process(queue = LongQueue)]
    async fn long(&self, job: LongCommand) -> anyhow::Result<()> {
        LONG.start(job.run);
        tokio::time::sleep(Duration::from_secs(6)).await;
        LONG.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, nest_rs_queue::QueueModule::for_root(None)],
    providers = [LongProcessor],
)]
struct LongModule;

/// An entry deleted by hand while a delivery holds it — no script deletes one
/// still pending — is gone with its job: the renewal that meets it says so,
/// naming the entry, rather than losing it in silence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_entry_deleted_while_delivered_is_said() {
    let logs = nest_rs_testing::LogCapture::install_global();
    let queue = <LongQueue as nest_rs_queue::Queue>::NAME;
    crate::forget(queue).await;
    let run = crate::this_run();
    let replica = crate::replica::<LongModule>().await;
    replica
        .producer
        .push(LongQueue, LongCommand { run }, None)
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || LONG.of(run).len() == 1).await;
    let pending: Vec<(String, String, u64, u64)> = redis::cmd("XPENDING")
        .arg(crate::key_of(queue, "jobs"))
        .arg(GROUP)
        .arg("-")
        .arg("+")
        .arg(10)
        .query_async(&mut crate::connect().await)
        .await
        .expect("XPENDING");
    let entry = pending[0].0.clone();
    let _: i64 = redis::cmd("XDEL")
        .arg(crate::key_of(queue, "jobs"))
        .arg(&entry)
        .query_async(&mut crate::connect().await)
        .await
        .expect("XDEL by hand");
    let said = "queue entries deleted while delivered; their jobs are gone, and a unique key they held stays held until cancel_unique frees it";
    crate::wait_until(Duration::from_secs(4), || {
        !logs.find(nest_rs_queue::TARGET, said).is_empty()
    })
    .await;
    replica.worker.shutdown().await.expect("clean shutdown");
    let line = logs.expect_one(nest_rs_queue::TARGET, said);
    assert_eq!(line.level, "warn");
    assert_eq!(line.field("backend_ids").as_deref(), Some(entry.as_str()));
    crate::forget(queue).await;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FlushedCommand {
    run: u64,
}

static FLUSHED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-script-flush", job = FlushedCommand)]
struct FlushedQueue;

#[injectable]
#[derive(Default)]
struct FlushedProcessor;

#[processor]
impl FlushedProcessor {
    #[process(queue = FlushedQueue)]
    async fn flushed(&self, job: FlushedCommand) -> anyhow::Result<()> {
        FLUSHED.start(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, nest_rs_queue::QueueModule::for_root(None)],
    providers = [FlushedProcessor],
)]
struct FlushedModule;

/// A Redis whose script cache was emptied — a restart, a `SCRIPT FLUSH` — still
/// runs the queue: each script is loaded again on its first `NOSCRIPT`, by the
/// client, and the job pushed after the flush runs once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queue_runs_on_after_its_scripts_are_flushed() {
    let queue = <FlushedQueue as nest_rs_queue::Queue>::NAME;
    crate::forget(queue).await;
    let run = crate::this_run();
    let replica = crate::replica::<FlushedModule>().await;
    let _: () = redis::cmd("SCRIPT")
        .arg("FLUSH")
        .query_async(&mut crate::connect().await)
        .await
        .expect("SCRIPT FLUSH");
    replica
        .producer
        .push(FlushedQueue, FlushedCommand { run }, None)
        .await
        .expect("a push after the flush");
    crate::wait_until(Duration::from_secs(10), || FLUSHED.of(run).len() == 1).await;
    replica.worker.shutdown().await.expect("clean shutdown");
    assert_eq!(FLUSHED.of(run).len(), 1, "the job ran once");
    assert_eq!(crate::filed(queue).await, 0, "and settled");
    crate::forget(queue).await;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PagedCommand {
    run: u64,
}

#[queue(name = "nestrs-e2e-paged", job = PagedCommand)]
struct PagedQueue;

static PAGED: Runs = Runs::new();

#[injectable]
struct PagedProcessor;

#[processor]
impl PagedProcessor {
    #[process(queue = PagedQueue, concurrency = 16, transactional = false)]
    async fn run(&self, job: PagedCommand) -> anyhow::Result<()> {
        PAGED.start(job.run);
        tokio::time::sleep(Duration::from_millis(200)).await;
        PAGED.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(paged_redis()), RedisQueueModule, nest_rs_queue::QueueModule::for_root(None)],
    providers = [PagedProcessor],
)]
struct PagedModule;

/// The suite's Redis at the framework's default budget: this case pushes,
/// reads and pages through ten thousand entries in single commands, where the
/// suite's half second is sized for one job's.
fn paged_redis() -> nest_rs_redis::RedisConfig {
    crate::at_default_budget(crate::redis_config())
}

/// Past ten thousand pending entries, a look for lapsed leases reads one page
/// of a thousand, and the next look reads on from where it stopped: its cost to
/// Redis stays a page, however many deliveries run elsewhere. Two lapsed
/// deliveries sit among ten thousand running ones, the first in the first page
/// and the second past it: the first look takes only the first, and a later one
/// the second. One look hands what it takes over in one receive, so its jobs
/// start together, while the next look comes after a blocking read of its own:
/// the two starts are told apart by the gap between them, never by when the
/// test happened to look.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_look_for_lapsed_leases_reads_one_page_and_the_next_reads_on() {
    let queue = <PagedQueue as nest_rs_queue::Queue>::NAME;
    crate::forget(queue).await;
    let run = crate::this_run();
    let (first, second, running) = (run, run + 1, run + 2);
    let producer = crate::producer_on(paged_redis()).await;
    producer
        .push(PagedQueue, PagedCommand { run: first }, None)
        .await
        .expect("the first lapsed job");
    let pushed = (0..999)
        .map(|_| running)
        .chain([second])
        .chain((0..9_001).map(|_| running));
    producer
        .push_many(PagedQueue, pushed.map(|run| PagedCommand { run }), None)
        .await
        .expect("the running jobs, the second lapsed one first past a page");
    let jobs = crate::key_of(queue, "jobs");
    let mut conn = crate::connect().await;
    let _: redis::Value = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(&jobs)
        .arg(GROUP)
        .arg(0)
        .query_async(&mut conn)
        .await
        .expect("the group");
    let _: redis::Value = redis::cmd("XREADGROUP")
        .arg("GROUP")
        .arg(GROUP)
        .arg("elsewhere")
        .arg("COUNT")
        .arg(20_000)
        .arg("STREAMS")
        .arg(&jobs)
        .arg(">")
        .query_async(&mut conn)
        .await
        .expect("every job delivered to a worker elsewhere");
    let first_pages: Vec<(String, Vec<String>)> = redis::cmd("XRANGE")
        .arg(&jobs)
        .arg("-")
        .arg("+")
        .arg("COUNT")
        .arg(1001)
        .query_async(&mut conn)
        .await
        .expect("XRANGE");
    for (entry, _) in [&first_pages[0], &first_pages[1000]] {
        let _: redis::Value = redis::cmd("XCLAIM")
            .arg(&jobs)
            .arg(GROUP)
            .arg("elsewhere")
            .arg(0)
            .arg(entry)
            .arg("IDLE")
            .arg(60_000)
            .arg("JUSTID")
            .query_async(&mut conn)
            .await
            .expect("its lease lapsed");
    }

    let app = TestApp::builder()
        .provide(RedisQueueConfig {
            lease: Duration::from_secs(30),
        })
        .module::<PagedModule>()
        .build_headless()
        .await
        .expect("the worker app boots");
    let worker = app
        .spawn_transport(nest_rs_queue::QueueWorker::new())
        .await
        .expect("the worker starts");
    crate::wait_until(Duration::from_secs(10), || {
        PAGED.finished(first) == 1 && PAGED.finished(second) == 1
    })
    .await;
    worker.shutdown().await.expect("clean shutdown");
    let gap = PAGED.of(second)[0].duration_since(PAGED.of(first)[0]);
    assert!(
        gap >= Duration::from_millis(500),
        "the delivery past the first page waits for the next look, where one look \
         would have started both together: {gap:?} apart"
    );
    assert!(PAGED.of(running).is_empty(), "no running delivery is taken");
    crate::forget(queue).await;
}

/// What holds on one server alone: a double in front of it stands where a
/// Sentinel or Cluster deployment has several hosts.
mod standalone {
    use super::*;

    /// A settle that ran on Redis and whose answer was lost: the worker cannot
    /// tell, and says so at `error`, but the job ended in the script that settled
    /// it — it never runs again, and the stream holds nothing of it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_settle_whose_answer_is_lost_still_ended_the_job_once() {
        let logs = nest_rs_testing::LogCapture::install_global();
        let queue = <GatedQueue as nest_rs_queue::Queue>::NAME;
        crate::forget(queue).await;
        let run = crate::this_run();
        let proxy = crate::MutingProxy::start().await;
        let replica = crate::replica_on::<GatedModule>(crate::through(proxy.url())).await;
        crate::producer()
            .await
            .push(GatedQueue, GatedCommand { run }, None)
            .await
            .expect("enqueue");
        crate::wait_until(Duration::from_secs(10), || GATED.of(run).len() == 1).await;

        proxy.mute();
        GATE.add_permits(1);
        crate::wait_until(Duration::from_secs(10), || GATED.finished(run) == 1).await;
        let unconfirmed = "job outcome not confirmed; unless it was written, the job runs again once its lease lapses";
        // The worker said it could not confirm the outcome.
        crate::wait_until(Duration::from_secs(10), || {
            !logs.find(nest_rs_queue::TARGET, unconfirmed).is_empty()
        })
        .await;
        // Past the lease, a delivery the settle had not ended would be reclaimed.
        tokio::time::sleep(crate::LEASE * 3).await;

        assert_eq!(GATED.of(run).len(), 1, "the job ran once");
        assert_eq!(crate::filed(queue).await, 0, "the settle ended it on Redis");
        drop(replica);
        crate::forget(queue).await;
    }
}
