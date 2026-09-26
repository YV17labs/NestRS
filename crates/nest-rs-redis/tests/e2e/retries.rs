//! The port's retry budget and its backoff, against a live worker.
//!
//! The port counts the budget and times the wait; the worker only honours what
//! it is told. This backend declares no delayed delivery, so the wait before a
//! job's next attempt passes in process, inside the delivery that fetched it —
//! and apalis never retries on its own, since a second count kept where the
//! method's is not would run a job more times than it declared. Three things
//! only a live worker shows, one test each:
//!
//! - a retryable failure runs again after the backoff, as the same job;
//! - a spent budget dead-letters the job once, and nothing runs it again;
//! - a shutdown during the wait gives up the wait and never the job: the next
//!   attempt runs on whichever replica takes it, still the same job.
//!
//! Every assertion follows the job by the id its push returned, and every
//! handler counts by a marker this run chose: queue names are compile-time
//! literals, so a run killed mid-job leaves work behind for the next one, and a
//! count that read it would be measuring an earlier run.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use nest_rs_core::{injectable, module, operation_log};
use nest_rs_queue::{JobId, JobProducerExt, PushReceipt, processor, queue, unit};
use nest_rs_redis::{RedisModule, RedisQueueModule, RedisWorkerModule};
use nest_rs_testing::LogCapture;
use serde::{Deserialize, Serialize};

/// The shortest wait the port's backoff allows after a first failed attempt:
/// one second, jittered down by at most a fifth.
const FIRST_WAIT_FLOOR: Duration = Duration::from_millis(800);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RetryCommand {
    run: u64,
}

/// A marker no earlier run of this suite chose.
fn this_run() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or_default()
        ^ u64::from(std::process::id())
}

/// When each attempt at this run's job started, one list per test.
struct Attempts(Mutex<Vec<(u64, Instant)>>);

impl Attempts {
    const fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    /// Record an attempt at `run`'s job, answering which attempt it is from 1.
    fn record(&self, run: u64) -> usize {
        let mut seen = self.0.lock().expect("attempts lock");
        seen.push((run, Instant::now()));
        seen.iter().filter(|(of, _)| *of == run).count()
    }

    fn of(&self, run: u64) -> Vec<Instant> {
        self.0
            .lock()
            .expect("attempts lock")
            .iter()
            .filter(|(of, _)| *of == run)
            .map(|(_, at)| *at)
            .collect()
    }
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

/// The job id an event names, when it names one.
fn names(event: &nest_rs_testing::CapturedEvent, id: &JobId) -> bool {
    event.field("job_id").as_deref() == Some(id.to_string().as_str())
}

// --- a retryable failure runs again after the backoff ---------------------------

static FLAKY: Attempts = Attempts::new();

#[queue(name = "nestrs-e2e-retries-flaky", job = RetryCommand)]
struct FlakyQueue;

#[injectable]
#[derive(Default)]
struct FlakyProcessor;

#[processor]
impl FlakyProcessor {
    #[process(queue = FlakyQueue, retries = 2)]
    async fn run(&self, job: RetryCommand) -> anyhow::Result<()> {
        if FLAKY.record(job.run) == 1 {
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
    let run = this_run();
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

static DOOMED: Attempts = Attempts::new();

#[queue(name = "nestrs-e2e-retries-spent", job = RetryCommand)]
struct DoomedQueue;

#[injectable]
#[derive(Default)]
struct DoomedProcessor;

#[processor]
impl DoomedProcessor {
    #[process(queue = DoomedQueue, retries = 1)]
    async fn run(&self, job: RetryCommand) -> anyhow::Result<()> {
        DOOMED.record(job.run);
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
    let run = this_run();
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

// --- a shutdown during the wait hands the job back ------------------------------

static HANDED: Attempts = Attempts::new();

#[queue(name = "nestrs-e2e-retries-hand-back", job = RetryCommand)]
struct HandBackQueue;

#[injectable]
#[derive(Default)]
struct HandBackProcessor;

#[processor]
impl HandBackProcessor {
    #[process(queue = HandBackQueue, retries = 3)]
    async fn run(&self, job: RetryCommand) -> anyhow::Result<()> {
        if HANDED.record(job.run) == 1 {
            anyhow::bail!("the upstream timed out");
        }
        Ok(())
    }
}

#[module(
    imports = [RedisModule::for_root(crate::redis_config()), RedisQueueModule, RedisWorkerModule::for_root(None)],
    providers = [HandBackProcessor],
)]
struct HandBackModule;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shutdown_during_the_wait_hands_the_job_back_for_its_next_attempt() {
    let logs = LogCapture::install_global();
    let run = this_run();
    let first = crate::replica::<HandBackModule>().await;
    let receipt = first
        .producer
        .push(HandBackQueue, RetryCommand { run }, None)
        .await
        .expect("enqueue");

    // The first attempt failed, and its replica is waiting out the backoff.
    crate::wait_until(Duration::from_secs(15), || {
        !lines_of(&logs, &receipt).is_empty()
    })
    .await;
    first
        .worker
        .shutdown()
        .await
        .expect("the wait gives way to the shutdown");

    let handed_back = logs.find(
        nest_rs_queue::TARGET,
        "job handed back at shutdown for its next attempt",
    );
    let handed_back = handed_back
        .iter()
        .find(|event| names(event, receipt.id()))
        .unwrap_or_else(|| panic!("the job was handed back, not dropped: {handed_back:#?}"));
    assert_eq!(
        handed_back.field("attempt").as_deref(),
        Some("2"),
        "it is handed back as the attempt it was waiting for",
    );

    let second = crate::replica::<HandBackModule>().await;
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
        "the replica that took it over ran the next attempt of the same job",
    );
    assert_eq!(HANDED.of(run).len(), 2, "and nothing ran it a third time");
}
