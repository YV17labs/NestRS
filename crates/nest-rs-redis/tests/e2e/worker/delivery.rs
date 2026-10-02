//! One delivery of a job, against a live worker: the port's retry budget and its
//! backoff as the worker honours them, and a delivery that loses its connection
//! halfway.
//!
//! The port counts the budget and times the wait; the worker only honours what
//! it is told. This backend declares delayed delivery, so the wait before a
//! job's next attempt is not spent in process: the next attempt is filed on the
//! queue's schedule, due once the backoff has passed, and the delivery ends —
//! and apalis never retries on its own, since a second count kept where the
//! method's is not would run a job more times than it declared. Four things
//! only a live worker shows, one test each:
//!
//! - a retryable failure runs again after the backoff, as the same job;
//! - a spent budget dead-letters the job once, and nothing runs it again;
//! - a replica shutting down while a retry waits holds nothing: the next attempt
//!   is already on the schedule, and the replica that starts next runs it;
//! - a connection Redis drops while an attempt runs costs the job nothing — it
//!   completes once, and a replica starting afterwards does not run it again.
//!
//! **apalis's count never ends a job.** apalis counts every delivery of a
//! record, hand-backs included, and kills a record answered with a plain error
//! once that count reaches its cap — five by default — with no event of the
//! port's. Two tests take a job past the cap on hand-backs alone, by holding
//! its lease the way a running delivery would: a hand-back Redis then refuses
//! answers apalis with the plain error, and the job still runs; and a job whose
//! budget is spent is dead-lettered by the port, once.
//!
//! **A job a newer release sealed is handed back, never dead-lettered.** An
//! older replica meeting it during a rolling deploy runs nothing, spends no
//! attempt, and files the record back as it was stored.
//!
//! **An attempt that never returns spends the budget too.** The envelope counts
//! the attempts that answered; the adapter counts the ones that started, so a
//! job whose attempt takes its replica down is dead-lettered once those spend
//! its budget, rather than crash-looping every replica that starts.
//!
//! Every assertion follows the job by the id its push returned, and every
//! handler counts by a marker this run chose.

use std::time::{Duration, Instant};

use nest_rs_core::{injectable, module, operation_log};
use nest_rs_queue::{JobId, JobProducerExt, PushOptions, PushReceipt, processor, queue, unit};
use nest_rs_redis::{
    RedisConfig, RedisConnection, RedisModule, RedisQueueModule, RedisWorkerModule,
};
use nest_rs_testing::LogCapture;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{DB_CONNECTION_RESET_MID_ATTEMPT, Runs};

/// The shortest wait the port's backoff allows after a first failed attempt:
/// one second, jittered down by at most a fifth.
const FIRST_WAIT_FLOOR: Duration = Duration::from_millis(800);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RetryCommand {
    run: u64,
}

/// The operation lines the attempts at the job `receipt` names filed, in the
/// order they ran: each one's `attempt` and `outcome`.
fn lines_of(logs: &LogCapture, receipt: &PushReceipt) -> Vec<(String, String)> {
    let id = receipt.id().to_string();
    logs.find(operation_log::TARGET, unit::JOB.name())
        .into_iter()
        .filter(|line| line.field("job_id").as_deref() == Some(id.as_str()))
        .map(|line| {
            (
                line.field("attempt").unwrap_or_default(),
                line.field("outcome").unwrap_or_default(),
            )
        })
        .collect()
}

fn line(attempt: u32, outcome: &str) -> (String, String) {
    (attempt.to_string(), outcome.to_owned())
}

// --- a retryable failure runs again after the backoff ---------------------------

static FLAKY: Runs = Runs::new();

#[queue(name = "nestrs-e2e-retries-flaky", job = RetryCommand)]
struct FlakyQueue;

#[injectable]
#[derive(Default)]
struct FlakyProcessor;

#[processor]
impl FlakyProcessor {
    #[process(queue = FlakyQueue, retries = 2)]
    async fn run(&self, job: RetryCommand) -> anyhow::Result<()> {
        if FLAKY.start(job.run) == 1 {
            anyhow::bail!("the upstream timed out");
        }
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [FlakyProcessor],
)]
struct FlakyModule;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retryable_failure_runs_again_after_the_backoff_as_the_same_job() {
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let replica = crate::replica::<FlakyModule>().await;
    let receipt = replica
        .producer
        .push(FlakyQueue, RetryCommand { run }, None)
        .await
        .expect("enqueue");

    crate::wait_until(Duration::from_secs(15), || {
        lines_of(&logs, &receipt).len() == 2
    })
    .await;
    replica
        .worker
        .shutdown()
        .await
        .expect("clean worker shutdown");

    assert_eq!(
        lines_of(&logs, &receipt),
        [line(1, operation_log::ERROR), line(2, operation_log::OK)],
        "the job failed once and completed on its second attempt, as one job",
    );
    let started = FLAKY.of(run);
    assert_eq!(started.len(), 2, "the handler ran exactly twice");
    let waited = started[1] - started[0];
    assert!(
        waited >= FIRST_WAIT_FLOOR,
        "the second attempt waited out the port's backoff, not {waited:?}",
    );
}

// --- a spent budget dead-letters once ------------------------------------------

static DOOMED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-retries-spent", job = RetryCommand)]
struct DoomedQueue;

#[injectable]
#[derive(Default)]
struct DoomedProcessor;

#[processor]
impl DoomedProcessor {
    #[process(queue = DoomedQueue, retries = 1)]
    async fn run(&self, job: RetryCommand) -> anyhow::Result<()> {
        DOOMED.start(job.run);
        anyhow::bail!("the upstream is gone")
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [DoomedProcessor],
)]
struct DoomedModule;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spent_budget_dead_letters_the_job_once_and_apalis_never_runs_it_again() {
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let replica = crate::replica::<DoomedModule>().await;
    let receipt = replica
        .producer
        .push(DoomedQueue, RetryCommand { run }, None)
        .await
        .expect("enqueue");

    crate::wait_until(Duration::from_secs(15), || {
        lines_of(&logs, &receipt).len() == 2
    })
    .await;
    // Long enough for apalis to have run the job again had it re-queued it: a
    // retry of its own is scheduled for now, and moved onto the queue by the
    // worker's one-second scan of the scheduled set.
    tokio::time::sleep(Duration::from_secs(4)).await;
    replica
        .worker
        .shutdown()
        .await
        .expect("clean worker shutdown");

    assert_eq!(
        lines_of(&logs, &receipt),
        [line(1, operation_log::ERROR), line(2, operation_log::ERROR)],
        "`retries = 1` is two attempts, and nothing after the dead letter",
    );
    assert_eq!(
        DOOMED.of(run).len(),
        2,
        "the handler ran twice: a dead letter is apalis's `Abort`, which it kills \
         rather than re-queues under a budget of its own",
    );
    let spent = logs.find(
        nest_rs_queue::TARGET,
        "job dead-lettered: retry budget spent",
    );
    assert!(
        !spent.is_empty()
            && spent
                .iter()
                .all(|event| event.field("attempts").as_deref() == Some("2")),
        "the dead letter says the budget was spent, after two attempts: {spent:#?}",
    );
}

// --- a retry waiting on the schedule outlives its replica -----------------------

static FILED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-retries-hand-back", job = RetryCommand)]
struct FiledQueue;

#[injectable]
#[derive(Default)]
struct FiledProcessor;

#[processor]
impl FiledProcessor {
    #[process(queue = FiledQueue, retries = 3)]
    async fn run(&self, job: RetryCommand) -> anyhow::Result<()> {
        if FILED.start(job.run) == 1 {
            anyhow::bail!("the upstream timed out");
        }
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [FiledProcessor],
)]
struct FiledModule;

/// The next attempt is filed the moment the first one fails, so the replica
/// holds nothing while the backoff runs: it shuts down at once, and whichever
/// replica starts next runs the next attempt, of the same job.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retry_filed_before_a_shutdown_runs_on_the_replica_that_starts_next() {
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let first = crate::replica::<FiledModule>().await;
    let receipt = first
        .producer
        .push(FiledQueue, RetryCommand { run }, None)
        .await
        .expect("enqueue");

    // The first attempt failed, and its next one is on the schedule.
    crate::wait_until(Duration::from_secs(15), || {
        !lines_of(&logs, &receipt).is_empty()
    })
    .await;
    let stopping = Instant::now();
    first
        .worker
        .shutdown()
        .await
        .expect("nothing holds the shutdown");
    assert!(
        stopping.elapsed() < Duration::from_secs(1),
        "no wait in process held the shutdown: {:?}",
        stopping.elapsed(),
    );
    assert!(
        logs.find(nest_rs_queue::TARGET, "job filed for its next attempt")
            .iter()
            .any(|event| crate::names(event, receipt.id())
                && event.field("attempt").as_deref() == Some("2")),
        "the next attempt was filed as attempt 2",
    );

    let second = crate::replica::<FiledModule>().await;
    crate::wait_until(Duration::from_secs(15), || {
        lines_of(&logs, &receipt).len() == 2
    })
    .await;
    second
        .worker
        .shutdown()
        .await
        .expect("clean worker shutdown");

    assert_eq!(
        lines_of(&logs, &receipt),
        [line(1, operation_log::ERROR), line(2, operation_log::OK)],
        "the replica that started next ran the next attempt of the same job",
    );
    assert_eq!(FILED.of(run).len(), 2, "and nothing ran it a third time");
}

// --- a connection dropped under a running attempt --------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResetCommand {
    run: u64,
}

static RESET: Runs = Runs::new();

#[queue(name = "nestrs-e2e-delivery-reset", job = ResetCommand)]
struct ResetQueue;

#[injectable]
#[derive(Default)]
struct ResetProcessor;

#[processor]
impl ResetProcessor {
    #[process(queue = ResetQueue, retries = 2)]
    async fn run(&self, job: ResetCommand) -> anyhow::Result<()> {
        RESET.start(job.run);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        RESET.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(None), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [ResetProcessor],
)]
struct ResetModule;

/// Redis closes every connection the worker holds while an attempt runs — what
/// a failover or a restart does. The attempt is the handler's and runs on; the
/// lease, the settled mark and the acknowledgement reconnect behind it. The job
/// completes once, carries its settled mark, and a replica starting afterwards —
/// whose startup sweep would find it again had its acknowledgement been lost —
/// does not run it a second time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_dropped_under_a_running_attempt_costs_the_job_nothing() {
    let run = crate::this_run();
    let first =
        crate::replica_on::<ResetModule>(crate::redis_config_on(DB_CONNECTION_RESET_MID_ATTEMPT))
            .await;
    let receipt = first
        .producer
        .push(ResetQueue, ResetCommand { run }, None)
        .await
        .expect("enqueue");
    crate::wait_until(Duration::from_secs(10), || !RESET.of(run).is_empty()).await;

    let dropped = crate::drop_every_connection_on(DB_CONNECTION_RESET_MID_ATTEMPT).await;
    assert!(dropped > 0, "the worker's connections were there to drop");

    crate::wait_until(Duration::from_secs(10), || RESET.finished(run) == 1).await;
    let settled = format!(
        "{}:settled:{}",
        crate::namespace("nestrs-e2e-delivery-reset"),
        receipt.id()
    );
    let mut admin = nest_rs_redis::RedisConnection::connect(&crate::redis_config_on(
        DB_CONNECTION_RESET_MID_ATTEMPT,
    ))
    .await
    .expect("connect");
    let mut marked = false;
    for _ in 0..50 {
        marked = redis::cmd("EXISTS")
            .arg(&settled)
            .query_async::<bool>(&mut admin)
            .await
            .unwrap_or(false);
        if marked {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(marked, "the job carries its settled mark");

    let second =
        crate::replica_on::<ResetModule>(crate::redis_config_on(DB_CONNECTION_RESET_MID_ATTEMPT))
            .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    first.worker.shutdown().await.expect("clean shutdown");
    second.worker.shutdown().await.expect("clean shutdown");

    assert_eq!(RESET.of(run).len(), 1, "the job ran once");
    assert_eq!(RESET.finished(run), 1, "and completed once");
}

// --- apalis's count never ends a job --------------------------------------------

/// How many hand-backs a job is taken through before the step under test — past
/// apalis's cap of five deliveries.
const PAST_THE_CAP: usize = 6;

/// A lease held on `job` the way a running delivery holds one, renewed every
/// tenth of a second: every delivery of the job meanwhile finds it held, and
/// hands the job back.
struct HeldLease {
    stop: CancellationToken,
    holding: tokio::task::JoinHandle<()>,
}

impl HeldLease {
    fn hold(queue: &str, job: &JobId) -> Self {
        let key = crate::key_of(queue, "leases", &job.to_string());
        let stop = CancellationToken::new();
        let until = stop.clone();
        let holding = tokio::spawn(async move {
            let mut conn = crate::connect().await;
            while !until.is_cancelled() {
                let _: () = redis::cmd("SET")
                    .arg(&key)
                    .arg("a-peer/still-running")
                    .arg("PX")
                    .arg(400)
                    .query_async(&mut conn)
                    .await
                    .expect("SET the lease");
                tokio::select! {
                    () = until.cancelled() => {}
                    () = tokio::time::sleep(Duration::from_millis(100)) => {}
                }
            }
            let _: i64 = redis::cmd("DEL")
                .arg(&key)
                .query_async(&mut conn)
                .await
                .expect("DEL the lease");
        });
        Self { stop, holding }
    }

    /// Let go, and wait until the lease is gone.
    async fn release(self) {
        self.stop.cancel();
        self.holding.await.expect("the lease holder ends");
    }
}

/// How many times the job `id` was handed back because its lease was held.
fn handed_back(logs: &LogCapture, id: &JobId) -> usize {
    logs.find(
        nest_rs_queue::TARGET,
        "job delivered while another delivery runs it; handing it back",
    )
    .iter()
    .filter(|event| crate::names(event, id))
    .count()
}

/// How many records apalis killed onto `queue`'s dead set.
async fn dead(queue: &str) -> i64 {
    redis::cmd("ZCARD")
        .arg(format!("{}:dead", crate::namespace(queue)))
        .query_async(&mut crate::connect().await)
        .await
        .expect("ZCARD")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CappedCommand {
    run: u64,
}

// A hand-back Redis refuses, past apalis's cap.

const CAPPED_QUEUE: &str = "nestrs-e2e-delivery-capped";

static CAPPED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-delivery-capped", job = CappedCommand)]
struct CappedQueue;

#[injectable]
#[derive(Default)]
struct CappedProcessor;

#[processor]
impl CappedProcessor {
    #[process(queue = CappedQueue, retries = 0)]
    async fn run(&self, job: CappedCommand) -> anyhow::Result<()> {
        CAPPED.start(job.run);
        CAPPED.finish(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(None), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [CappedProcessor],
)]
struct CappedModule;

/// The ACL user the capped test's replicas reach Redis as, and its password — a
/// test fixture, never a secret.
const CAPPED_USER: &str = "nestrs-e2e-capped";
const CAPPED_PASSWORD: &str = "past-the-cap";

fn capped_config() -> RedisConfig {
    RedisConfig {
        url: crate::redis_url_on(0).replacen(
            "://",
            &format!("://{CAPPED_USER}:{CAPPED_PASSWORD}@"),
            1,
        ),
        ..Default::default()
    }
}

/// Give the capped user the queue's keys: every one of them, or every one but
/// the schedule — so a hand-back's filing is refused, while the fetch, the
/// guard and apalis's acknowledgement still reach Redis.
async fn grant(admin: &mut RedisConnection, schedule: bool) {
    let namespace = crate::namespace(CAPPED_QUEUE);
    let mut acl = redis::cmd("ACL");
    acl.arg("SETUSER").arg(CAPPED_USER).arg("resetkeys");
    if schedule {
        acl.arg(format!("~{namespace}:*"));
    } else {
        for structure in [
            "active",
            "consumers",
            "inflight:*",
            "data",
            "data::result",
            "signal",
            "dead",
            "done",
            "failed",
            "open:*",
            "leases:*",
            "settled:*",
            "cancelled:*",
            "checkpoints:*",
            "attempts:*",
            "deferred:*",
            "unique:*",
            "throttle",
        ] {
            acl.arg(format!("~{namespace}:{structure}"));
        }
    }
    let _: () = acl.query_async(admin).await.expect("ACL SETUSER");
}

/// A job handed back past apalis's cap, whose next hand-back Redis refuses, is
/// answered to apalis with a plain error — which apalis, counting the job's
/// sixth delivery, would have killed onto its dead set. The job's record lifts
/// the cap, so apalis files it back instead, and it runs.
///
/// The refusal is an ACL that denies the queue's schedule: the hand-back's
/// filing fails there, and so does apalis's own re-filing after it, which
/// leaves the job in flight until the next replica's startup sweep — while a
/// kill would have reached the dead set, which the ACL leaves open.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_handed_back_past_apalis_cap_still_runs_when_a_hand_back_fails() {
    let logs = LogCapture::install_global();
    let mut admin = crate::connect().await;
    let _: () = redis::cmd("ACL")
        .arg("SETUSER")
        .arg(CAPPED_USER)
        .arg("reset")
        .arg("on")
        .arg(format!(">{CAPPED_PASSWORD}"))
        .arg("+@all")
        .arg("-@dangerous")
        .query_async(&mut admin)
        .await
        .expect("ACL SETUSER");
    grant(&mut admin, true).await;

    let run = crate::this_run();
    let first = crate::replica_on::<CappedModule>(capped_config()).await;
    // Held back a second, so the lease is held before the first delivery.
    let receipt = first
        .producer
        .push(
            CappedQueue,
            CappedCommand { run },
            PushOptions::default().with_delay(Duration::from_secs(1)),
        )
        .await
        .expect("a delayed push");
    let lease = HeldLease::hold(CAPPED_QUEUE, receipt.id());
    crate::wait_until(Duration::from_secs(60), || {
        handed_back(&logs, receipt.id()) >= PAST_THE_CAP
    })
    .await;
    assert!(
        handed_back(&logs, receipt.id()) >= PAST_THE_CAP,
        "the job was handed back past apalis's cap"
    );

    // The schedule is closed to the worker, so the job's next hand-back is
    // refused; the test moves it onto the queue itself, through apalis.
    grant(&mut admin, false).await;
    let mut promoter: apalis_redis::RedisStorage<serde_json::Value, RedisConnection> =
        apalis_redis::RedisStorage::new_with_config(
            admin.clone(),
            apalis_redis::Config::default().set_namespace(&crate::namespace(CAPPED_QUEUE)),
        );
    let refused = "job not handed back; apalis files its stored record again, due at once, and \
                   it runs again";
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline
        && !logs
            .find(nest_rs_queue::TARGET, refused)
            .iter()
            .any(|event| crate::names(event, receipt.id()))
    {
        promoter
            .enqueue_scheduled(10)
            .await
            .expect("the admin moves due jobs onto the queue");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let failed = logs.find(nest_rs_queue::TARGET, refused);
    let failed = failed
        .iter()
        .find(|event| crate::names(event, receipt.id()))
        .unwrap_or_else(|| panic!("a hand-back of the job was refused: {failed:#?}"));
    assert_eq!(failed.level, "error");
    assert_eq!(failed.field("reason").as_deref(), Some("leased elsewhere"));
    // apalis answers the plain error on its heartbeat: a kill would land now.
    tokio::time::sleep(Duration::from_secs(2)).await;

    grant(&mut admin, true).await;
    lease.release().await;
    let second = crate::replica_on::<CappedModule>(capped_config()).await;
    crate::wait_until(Duration::from_secs(20), || CAPPED.finished(run) == 1).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    first.worker.shutdown().await.expect("clean shutdown");
    second.worker.shutdown().await.expect("clean shutdown");
    let killed = dead(CAPPED_QUEUE).await;
    let _: () = redis::cmd("ACL")
        .arg("DELUSER")
        .arg(CAPPED_USER)
        .query_async(&mut admin)
        .await
        .expect("ACL DELUSER");
    crate::forget(CAPPED_QUEUE).await;

    assert_eq!(
        killed, 0,
        "apalis killed nothing onto the dead set: the job's record lifts its cap"
    );
    assert_eq!(CAPPED.of(run).len(), 1, "the job ran, once");
    assert_eq!(CAPPED.finished(run), 1, "and completed");
}

// The port's budget, spent past apalis's cap.

const SPENT_QUEUE: &str = "nestrs-e2e-delivery-cap-spent";

static SPENT: Runs = Runs::new();

#[queue(name = "nestrs-e2e-delivery-cap-spent", job = CappedCommand)]
struct SpentQueue;

#[injectable]
#[derive(Default)]
struct SpentProcessor;

#[processor]
impl SpentProcessor {
    #[process(queue = SpentQueue, retries = 1)]
    async fn run(&self, job: CappedCommand) -> anyhow::Result<()> {
        SPENT.start(job.run);
        anyhow::bail!("the upstream is gone")
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [SpentProcessor],
)]
struct SpentModule;

/// A job handed back past apalis's cap still has its whole budget: `retries =
/// 1` is two attempts, and the second failure dead-letters the job once — the
/// port's event, and one record on apalis's dead set — whatever apalis counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_ports_budget_dead_letters_a_job_once_past_apalis_cap() {
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let replica = crate::replica::<SpentModule>().await;
    let receipt = replica
        .producer
        .push(
            SpentQueue,
            CappedCommand { run },
            PushOptions::default().with_delay(Duration::from_secs(1)),
        )
        .await
        .expect("a delayed push");
    let lease = HeldLease::hold(SPENT_QUEUE, receipt.id());
    crate::wait_until(Duration::from_secs(60), || {
        handed_back(&logs, receipt.id()) >= PAST_THE_CAP
    })
    .await;
    lease.release().await;
    assert!(
        handed_back(&logs, receipt.id()) >= PAST_THE_CAP,
        "the job was handed back past apalis's cap"
    );

    crate::wait_until(Duration::from_secs(20), || {
        lines_of(&logs, &receipt).len() == 2
    })
    .await;
    // Long enough for apalis to have filed the job again had it answered the
    // dead letter with a retry of its own.
    tokio::time::sleep(Duration::from_secs(3)).await;
    replica
        .worker
        .shutdown()
        .await
        .expect("clean worker shutdown");
    let killed = dead(SPENT_QUEUE).await;
    crate::forget(SPENT_QUEUE).await;

    assert_eq!(
        lines_of(&logs, &receipt),
        [line(1, operation_log::ERROR), line(2, operation_log::ERROR)],
        "the whole budget ran — two attempts — however many hand-backs came first",
    );
    assert_eq!(
        SPENT.of(run).len(),
        2,
        "and nothing ran the job a third time"
    );
    // The event names its job on the attempt's span, and this process runs this
    // test alone: every dead letter it saw is this job's.
    let spent = logs.find(
        nest_rs_queue::TARGET,
        "job dead-lettered: retry budget spent",
    );
    assert_eq!(
        spent.len(),
        1,
        "the port dead-lettered the job once: {spent:#?}"
    );
    assert_eq!(spent[0].field("attempts").as_deref(), Some("2"));
    assert_eq!(killed, 1, "one record on apalis's dead set");
}

// --- an attempt that never returns spends the budget -----------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CrashCommand {
    run: u64,
}

static CRASHED: Runs = Runs::new();

#[queue(name = "nestrs-e2e-delivery-crash", job = CrashCommand)]
struct CrashQueue;

#[injectable]
#[derive(Default)]
struct CrashProcessor;

#[processor]
impl CrashProcessor {
    /// Never answers: the replica running it is killed before it could.
    #[process(queue = CrashQueue, retries = 1)]
    async fn run(&self, job: CrashCommand) -> anyhow::Result<()> {
        CRASHED.start(job.run);
        std::future::pending::<()>().await;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [CrashProcessor],
)]
struct CrashModule;

/// A job whose attempt takes its replica down — an abort, an OOM kill, a hang
/// until the pod is killed — returns no answer, so the envelope never counts it,
/// and before the adapter counted starts itself such a job was delivered forever,
/// crash-looping every replica an autoscaler started. The attempts it starts
/// spend the budget instead: with `retries = 1` it runs twice, each run killed,
/// and the third delivery dead-letters it without running, saying two attempts
/// never returned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_whose_attempts_never_return_is_dead_lettered_once_they_spend_its_budget() {
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let mut replica = crate::mortal_replica::<CrashModule>().await;
    let receipt = replica
        .producer
        .push(CrashQueue, CrashCommand { run }, None)
        .await
        .expect("enqueue");
    for started in 1..=2 {
        crate::wait_until(Duration::from_secs(15), || CRASHED.of(run).len() == started).await;
        assert_eq!(CRASHED.of(run).len(), started, "attempt {started} started");
        replica.kill().await;
        replica = crate::mortal_replica::<CrashModule>().await;
    }

    let spent = "job dead-lettered: retry budget spent by attempts that never returned";
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && logs.find(nest_rs_queue::TARGET, spent).is_empty() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    replica.kill().await;

    let dead_letters = logs.find(nest_rs_queue::TARGET, spent);
    assert_eq!(
        dead_letters.len(),
        1,
        "the third delivery dead-letters the job, once: {dead_letters:#?}"
    );
    assert_eq!(dead_letters[0].level, "error");
    assert_eq!(dead_letters[0].field("unfinished").as_deref(), Some("2"));
    assert_eq!(dead_letters[0].field("attempts").as_deref(), Some("2"));
    // The replica is killed in process, by aborting its worker, which drops each
    // attempt where it waits: those file `cancelled`, where a process killed
    // outright files nothing. Either way neither answered, and both spent the
    // budget.
    assert_eq!(
        lines_of(&logs, &receipt),
        [
            line(1, operation_log::CANCELLED),
            line(2, operation_log::CANCELLED),
            line(3, operation_log::ERROR),
        ],
        "the killed attempts were stopped, not failed, and the dead letter is the third's",
    );
    assert_eq!(
        CRASHED.of(run).len(),
        2,
        "the budget of two attempts ran twice, and nothing ran a third",
    );
    let mark = crate::key_of(
        "nestrs-e2e-delivery-crash",
        "settled",
        &receipt.id().to_string(),
    );
    assert_eq!(crate::read(&mark).await.as_deref(), Some("dead-lettered"));
}

// --- a job a newer release sealed -------------------------------------------------

const NEWER_QUEUE: &str = "nestrs-e2e-delivery-newer";

/// The id a newer release's push returned, spelled as this release spells one
/// — minted in 2023, so a wait counted from the push would be years long.
const NEWER_JOB: &str = "01890a5d-ac96-774b-bcce-b302099a8057";

/// The trace a newer release's producer sealed, spelled as this release spells
/// one.
const NEWER_TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

/// File, as a newer release's push files it, a job under `id` on `queue`.
async fn file_a_newer_releases_job(queue: &str, id: &str, run: u64) -> apalis_redis::Config {
    use apalis::prelude::{Request, Storage};

    let newer = u64::from(nest_rs_queue::WIRE_FORMAT_VERSION) + 1;
    let apalis = apalis_redis::Config::default().set_namespace(&crate::namespace(queue));
    let mut storage: apalis_redis::RedisStorage<serde_json::Value, RedisConnection> =
        apalis_redis::RedisStorage::new_with_config(crate::connect().await, apalis.clone());
    storage
        .push_request(Request::new(serde_json::json!({
            "v": newer,
            "id": id,
            "attempt": 1,
            "traceparent": NEWER_TRACEPARENT,
            "payload": { "run": run },
        })))
        .await
        .expect("a newer producer's job, filed as apalis files one");
    apalis
}

static NEWER: Runs = Runs::new();

#[queue(name = "nestrs-e2e-delivery-newer", job = RetryCommand)]
struct NewerQueue;

#[injectable]
#[derive(Default)]
struct NewerProcessor;

#[processor]
impl NewerProcessor {
    #[process(queue = NewerQueue, throttle(limit = 5, window = "1m"))]
    async fn run(&self, job: RetryCommand) -> anyhow::Result<()> {
        NEWER.start(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [NewerProcessor],
)]
struct NewerModule;

/// A replica of this release meets a job a newer one sealed: nothing runs, no
/// attempt is counted, the record goes back to the schedule as it was stored,
/// and the line names the job by the id its push returned, in the trace its
/// producer sealed. It was dead-lettered, under an id minted for the delivery:
/// a rolling deploy lost every such job an old replica fetched. Nor does it keep
/// a start against its method's throttle: every deferral of a newer job used
/// to, so a rolling deploy shrank the window the jobs this release can run were
/// left. Its wait is counted from this first hand-back, in Redis's own record —
/// never from its push, which is years old, so a job pushed with a delay is
/// not charged for the delay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_a_newer_release_sealed_is_handed_back_as_stored_within_the_patience() {
    let logs = LogCapture::install_global();
    crate::forget(NEWER_QUEUE).await;
    let run = crate::this_run();
    let newer = u64::from(nest_rs_queue::WIRE_FORMAT_VERSION) + 1;
    let apalis = file_a_newer_releases_job(NEWER_QUEUE, NEWER_JOB, run).await;

    let replica = crate::replica::<NewerModule>().await;
    let job = JobId::parse(NEWER_JOB).expect("a job id");
    crate::wait_until(Duration::from_secs(15), || {
        logs.find(
            nest_rs_queue::TARGET,
            "job handed back for a consumer of a newer release",
        )
        .iter()
        .any(|event| crate::names(event, &job))
    })
    .await;
    replica
        .worker
        .shutdown()
        .await
        .expect("clean worker shutdown");

    let warned: Vec<_> = logs
        .find(
            nest_rs_queue::TARGET,
            "job sealed by a newer release handed back unread",
        )
        .into_iter()
        .filter(|event| crate::names(event, &job))
        .collect();
    assert_eq!(warned.len(), 1, "one delivery, one line: {warned:#?}");
    assert_eq!(warned[0].field("version"), Some(newer.to_string()));
    assert_eq!(
        warned[0].trace_id.as_deref(),
        Some("4bf92f3577b34da6a3ce929d0e0e4736"),
        "in the trace the newer producer sealed",
    );
    assert!(
        warned[0]
            .field("waited_ms")
            .and_then(|waited| waited.parse::<u64>().ok())
            .is_some_and(|waited| waited < 60_000),
        "the wait is counted from the first hand-back, not the push: {:?}",
        warned[0].field("waited_ms"),
    );
    assert!(NEWER.of(run).is_empty(), "nothing ran");
    assert_eq!(dead(NEWER_QUEUE).await, 0, "nothing was dead-lettered");
    let first_handed_back = crate::read(&crate::key_of(NEWER_QUEUE, "deferred", NEWER_JOB)).await;
    assert!(
        first_handed_back
            .as_deref()
            .and_then(|at| at.parse::<u64>().ok())
            .is_some(),
        "the first hand-back is recorded, in Redis's milliseconds: {first_handed_back:?}",
    );
    let started: Option<i64> = redis::cmd("GET")
        .arg(crate::key_of(NEWER_QUEUE, "attempts", NEWER_JOB))
        .query_async(&mut crate::connect().await)
        .await
        .expect("GET");
    assert!(
        started.unwrap_or(0) == 0,
        "no attempt is counted: {started:?}"
    );
    let throttled: Option<i64> = redis::cmd("GET")
        .arg(format!("{}:throttle", crate::namespace(NEWER_QUEUE)))
        .query_async(&mut crate::connect().await)
        .await
        .expect("GET");
    assert_eq!(
        throttled.unwrap_or(0),
        0,
        "no start is left counted against the throttle's window",
    );
    let scheduled: Vec<String> = redis::cmd("ZRANGE")
        .arg(apalis.scheduled_jobs_set())
        .arg(0)
        .arg(-1)
        .query_async(&mut crate::connect().await)
        .await
        .expect("ZRANGE");
    assert_eq!(scheduled.len(), 1, "the job waits on the schedule");
    let stored: Option<String> = redis::cmd("HGET")
        .arg(apalis.job_data_hash())
        .arg(&scheduled[0])
        .query_async(&mut crate::connect().await)
        .await
        .expect("HGET");
    let stored = stored.expect("its record is kept");
    assert!(
        stored.contains(&format!("\"v\":{newer}")) && stored.contains(NEWER_JOB),
        "the record is the newer release's, as stored: {stored}"
    );

    crate::forget(NEWER_QUEUE).await;
}

const PATIENCE_QUEUE: &str = "nestrs-e2e-delivery-newer-patience";

static PATIENCE: Runs = Runs::new();

#[queue(name = "nestrs-e2e-delivery-newer-patience", job = RetryCommand)]
struct PatienceQueue;

#[injectable]
#[derive(Default)]
struct PatienceProcessor;

#[processor]
impl PatienceProcessor {
    #[process(queue = PatienceQueue)]
    async fn run(&self, job: RetryCommand) -> anyhow::Result<()> {
        PATIENCE.start(job.run);
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(crate::brisk())],
    providers = [PatienceProcessor],
)]
struct PatienceModule;

/// A newer release's job handed back unread for longer than the patience — its
/// first hand-back recorded a day and an hour ago — is dead-lettered by the
/// next replica of this release that meets it rather than handed back forever:
/// said at `error` naming both versions, its unit line filed, its record kept in
/// the dead set for a consumer of that release, and nothing it held left behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_a_newer_release_sealed_is_dead_lettered_once_it_waited_unread_past_the_patience() {
    let logs = LogCapture::install_global();
    crate::forget(PATIENCE_QUEUE).await;
    let run = crate::this_run();
    let id = uuid::Uuid::now_v7().to_string();
    let apalis = file_a_newer_releases_job(PATIENCE_QUEUE, &id, run).await;
    let mut admin = crate::connect().await;
    let (secs, micros): (u64, u64) = redis::cmd("TIME")
        .query_async(&mut admin)
        .await
        .expect("TIME");
    let waited = nest_rs_queue::consume::NEWER_RELEASE_PATIENCE + Duration::from_secs(3600);
    let first_handed_back =
        secs * 1000 + micros / 1000 - u64::try_from(waited.as_millis()).expect("milliseconds");
    let deferred = crate::key_of(PATIENCE_QUEUE, "deferred", &id);
    let _: () = redis::cmd("SET")
        .arg(&deferred)
        .arg(first_handed_back)
        .query_async(&mut admin)
        .await
        .expect("a first hand-back a day and an hour ago");

    let replica = crate::replica::<PatienceModule>().await;
    let said = "job dead-lettered: a newer release sealed it, and none of its consumers ran it in \
                time";
    let deadline = Instant::now() + Duration::from_secs(15);
    while dead(PATIENCE_QUEUE).await == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    replica
        .worker
        .shutdown()
        .await
        .expect("clean worker shutdown");

    let dead_lettered = logs.find(nest_rs_queue::TARGET, said);
    assert_eq!(dead_lettered.len(), 1, "{dead_lettered:#?}");
    assert_eq!(dead_lettered[0].level, "error");
    assert_eq!(
        dead_lettered[0].field("version"),
        Some((u64::from(nest_rs_queue::WIRE_FORMAT_VERSION) + 1).to_string())
    );
    assert!(PATIENCE.of(run).is_empty(), "nothing ran");
    assert_eq!(dead(PATIENCE_QUEUE).await, 1, "the job is in the dead set");
    let kept: Option<String> = redis::cmd("HGET")
        .arg(apalis.job_data_hash())
        .arg(
            redis::cmd("ZRANGE")
                .arg(apalis.dead_jobs_set())
                .arg(0)
                .arg(0)
                .query_async::<Vec<String>>(&mut admin)
                .await
                .expect("ZRANGE")
                .first()
                .expect("the dead job's task"),
        )
        .query_async(&mut admin)
        .await
        .expect("HGET");
    assert!(
        kept.is_some_and(|record| record.contains(&id)),
        "the newer release's record is kept for a consumer of that release",
    );
    assert_eq!(
        crate::read(&deferred).await,
        None,
        "the record of its wait went with it"
    );
    crate::forget(PATIENCE_QUEUE).await;
}

// --- a dead-letter record is the failure as its line renders it -------------------

static RECORDED: Runs = Runs::new();

/// The queue whose job reads a reply it cannot decode.
const RECORDED_QUEUE: &str = "nestrs-e2e-dead-record";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReplyCommand {
    run: u64,
    reply: String,
}

#[queue(name = "nestrs-e2e-dead-record", job = ReplyCommand)]
struct RecordedQueue;

#[injectable]
#[derive(Default)]
struct RecordedProcessor;

#[processor]
impl RecordedProcessor {
    #[process(queue = RecordedQueue)]
    async fn run(&self, job: ReplyCommand) -> anyhow::Result<()> {
        use anyhow::Context;
        RECORDED.start(job.run);
        let _amount: u64 =
            serde_json::from_str(&job.reply).context("reading the upstream reply")?;
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [RecordedProcessor],
)]
struct RecordedModule;

/// What apalis keeps as the reason of a job it killed onto the dead set is the
/// sentence the dead-letter line carries: every cause a `.context(…)` wrapped,
/// and a decode failure the body returned said without the value. It was the
/// error's source, verbatim — the context alone, or serde's sentence quoting a
/// secret into Redis for as long as the record is kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dead_letter_record_says_a_decode_failure_as_its_line_does() {
    const SAID: &str =
        "reading the upstream reply: invalid type: a string, expected u64 at line 1 column 24";
    let logs = LogCapture::install_global();
    let run = crate::this_run();
    let replica = crate::replica::<RecordedModule>().await;
    let receipt = replica
        .producer
        .push(
            RecordedQueue,
            ReplyCommand {
                run,
                reply: r#""sk_live_51HsecretTOKEN""#.to_owned(),
            },
            None,
        )
        .await
        .expect("enqueue");

    // apalis keeps the reason under its own id for the record, which the job's
    // line carries as `backend_id`; read that record and no other.
    let id = receipt.id().to_string();
    let backend_id = || {
        logs.find(operation_log::TARGET, unit::JOB.name())
            .into_iter()
            .find(|line| line.field("job_id").as_deref() == Some(id.as_str()))
            .and_then(|line| line.field("backend_id"))
    };
    crate::wait_until(Duration::from_secs(15), || backend_id().is_some()).await;
    let backend_id = backend_id().expect("the job filed its line");
    let results = format!("{}:data::result", crate::namespace(RECORDED_QUEUE));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut record: Option<String> = None;
    while record.is_none() && tokio::time::Instant::now() < deadline {
        record = redis::cmd("HGET")
            .arg(&results)
            .arg(&backend_id)
            .query_async(&mut crate::connect().await)
            .await
            .expect("HGET");
        if record.is_none() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    replica
        .worker
        .shutdown()
        .await
        .expect("clean worker shutdown");

    assert_eq!(RECORDED.of(run).len(), 1, "the job ran once");
    let record = record.expect("the dead-lettered job's reason is kept");
    assert!(record.contains(SAID), "{record}");
    assert!(!record.contains("sk_live"), "{record}");
    let said = logs.expect_one(
        nest_rs_queue::TARGET,
        "job dead-lettered: retry budget spent",
    );
    assert_eq!(said.field("error").as_deref(), Some(SAID));
    assert!(
        logs.events()
            .iter()
            .all(|line| !line.fields.values().any(|value| value.contains("sk_live"))),
        "no line quotes the value",
    );

    crate::forget(RECORDED_QUEUE).await;
}
