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
use nest_rs_redis::{RedisModule, RedisQueueConfig, RedisQueueModule};
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
        Self::idle(queue, Some("nestrs-kit-ghost")).await
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
    for _ in 0..200 {
        if ENDED.finished(run) == 1 && dead_letters().await == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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
    let mut held = None;
    for _ in 0..40 {
        held = crate::field_of(queue, "checkpoints", &job).await;
        if held.as_deref() == Some("2") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        held.as_deref(),
        Some("2"),
        "the second attempt's save is in Redis"
    );

    doomed.kill().await;
    let heir = crate::replica::<ImportModule>().await;
    crate::wait_until(Duration::from_secs(20), || IMPORTED.finished(run) == 1).await;
    let mut cleared = false;
    for _ in 0..40 {
        if crate::field_of(queue, "checkpoints", &job).await.is_none() {
            cleared = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    heir.worker.shutdown().await.expect("clean shutdown");

    assert_eq!(
        seen(run),
        [None, Some(1), Some(2)],
        "each attempt resumed from the last save: after a retry, and after a dead replica",
    );
    assert_eq!(IMPORTED.finished(run), 1, "the job completed once");
    assert!(cleared, "its checkpoint went with it");
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
    assert!(
        span >= Duration::from_millis(1900),
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

// --- a cancel while a delivery runs --------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HeldCommand {
    run: u64,
}

static HELD: Runs = Runs::new();

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
        tokio::time::sleep(Duration::from_millis(1500)).await;
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

/// The consumers of `queue`'s group.
async fn consumers(queue: &str) -> Vec<String> {
    let consumers: Vec<std::collections::HashMap<String, redis::Value>> = redis::cmd("XINFO")
        .arg("CONSUMERS")
        .arg(crate::key_of(queue, "jobs"))
        .arg(GROUP)
        .query_async(&mut crate::connect().await)
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

/// A stream deleted under a running worker — by hand, or a flush — takes its
/// group with it; the worker makes it again and runs what is filed next. An
/// idle worker's read blocks on a connection of its own, so a push answers at
/// once meanwhile; and a worker that stops leaves the group.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_group_is_made_again_after_its_stream_went_and_a_stopped_worker_leaves_it() {
    let queue = <PlainQueue as nest_rs_queue::Queue>::NAME;
    crate::forget(queue).await;
    let run = crate::this_run();
    let replica = crate::replica::<PlainModule>().await;
    replica
        .producer
        .push(PlainQueue, PlainCommand { run }, None)
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || PLAIN.of(run).len() == 1).await;
    assert_eq!(
        consumers(queue).await.len(),
        1,
        "the worker reads as one consumer"
    );

    let clients: String = redis::cmd("CLIENT")
        .arg("LIST")
        .query_async(&mut crate::connect().await)
        .await
        .expect("CLIENT LIST");
    assert!(
        clients
            .lines()
            .any(|client| client.contains("cmd=xreadgroup")),
        "an idle worker's read blocks on a connection: {clients}",
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

    crate::forget(queue).await;
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
        consumers(queue).await.is_empty(),
        "a worker that stopped holding nothing leaves the group",
    );
    crate::forget(queue).await;
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
    let replica = crate::replica_on::<GatedModule>(nest_rs_redis::RedisConfig {
        url: proxy.url(),
        connect_timeout: Duration::from_secs(1),
        ..Default::default()
    })
    .await;
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
    for _ in 0..200 {
        if !logs.find(nest_rs_queue::TARGET, unconfirmed).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Past the lease, a delivery the settle had not ended would be reclaimed.
    tokio::time::sleep(crate::LEASE * 3).await;

    assert_eq!(GATED.of(run).len(), 1, "the job ran once");
    assert!(
        !logs.find(nest_rs_queue::TARGET, unconfirmed).is_empty(),
        "the worker said it could not confirm the outcome",
    );
    assert_eq!(crate::filed(queue).await, 0, "the settle ended it on Redis");
    drop(replica);
    crate::forget(queue).await;
}

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

/// The script the delivery page prints to file a dead letter back, read off
/// the page as an operator copies it.
fn documented_refile() -> String {
    let page = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/src/content/docs/queue/delivery.mdx");
    let text = std::fs::read_to_string(&page).expect("the delivery page reads");
    let line = text
        .lines()
        .find(|line| line.starts_with("redis-cli EVAL \""))
        .expect("the delivery page prints the command that files a dead letter back");
    let script = &line["redis-cli EVAL \"".len()..];
    script[..script.find('"').expect("the script is quoted")].to_owned()
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
    let mut letters = Vec::new();
    for _ in 0..200 {
        letters = dead().await;
        if !letters.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(letters.len(), 1, "the job dead-lettered");

    FIXED.store(true, std::sync::atomic::Ordering::SeqCst);
    let refiled: Option<String> = redis::Script::new(&documented_refile())
        .key(crate::key_of(queue, "dead"))
        .key(crate::key_of(queue, "jobs"))
        .key(crate::key_of(queue, "entries"))
        .arg(&letters[0].0)
        .invoke_async(&mut crate::connect().await)
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
