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
//! Every assertion follows the job by the id its push returned, and every
//! handler counts by a marker this run chose.

use std::time::{Duration, Instant};

use nest_rs_core::{injectable, module, operation_log};
use nest_rs_queue::{JobProducerExt, PushReceipt, processor, queue, unit};
use nest_rs_redis::{RedisModule, RedisQueueModule, RedisWorkerModule};
use nest_rs_testing::LogCapture;
use serde::{Deserialize, Serialize};

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
    logs.find(operation_log::TARGET, unit::JOB)
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
