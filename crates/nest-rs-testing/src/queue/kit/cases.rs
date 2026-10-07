//! The kit's cases: each pushes jobs through the backend's producer, runs them
//! on the port's [`QueueWorker`] over the backend's consumer, and asserts what
//! the contract promises — read off what the jobs' attempts reported and the
//! lines the port filed.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nest_rs_core::{Correlation, Transport, current_trace_id, operation_log, with_request_scope};
use nest_rs_queue::{
    Capability, Destination, JobProducer, JobProducerExt, PushOptions, PushReceipt, Queue,
    QueueConfig, QueueName, QueueWorker, TARGET, unit,
};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::KitBackend;
use super::command::{
    Act, BudgetQueue, ConcurrencyQueue, DeathQueue, DelayQueue, DrainQueue, KitCommand, OnceQueue,
    RenewalQueue, RetryQueue, StallQueue, TakenQueue, TraceQueue,
};
use super::module::CaseModule;
use super::probe::{self, Attempt};
use crate::{HeadlessApp, LogCapture};

/// The drain window of a case's workers.
const WINDOW: Duration = Duration::from_secs(2);

/// The longest a case waits for what it expects, past the leases it waits out.
const PATIENCE: Duration = Duration::from_secs(20);

/// A job pushed once runs once and completes.
pub async fn a_pushed_job_runs_once<B: KitBackend>(backend: B, logs: LogCapture) {
    let kit = Kit::begin(backend, logs, OnceQueue).await;
    let worker = kit.worker(WINDOW).await;
    kit.push(&worker, OnceQueue, 0, Act::Complete, None).await;
    kit.until(|| kit.completed(OnceQueue::NAME, 0) == 1).await;
    worker.stop().await;
    assert_eq!(kit.attempts(OnceQueue::NAME, 0).len(), 1, "one attempt");
}

/// A method declaring `concurrency = 3` runs three of its jobs at once, and
/// never four.
pub async fn a_method_runs_no_more_attempts_at_once_than_its_concurrency<B: KitBackend>(
    backend: B,
    logs: LogCapture,
) {
    let kit = Kit::begin(backend, logs, ConcurrencyQueue).await;
    let worker = kit.worker(WINDOW).await;
    for seq in 0..6 {
        kit.push(&worker, ConcurrencyQueue, seq, Act::Hold { ms: 300 }, None)
            .await;
    }
    kit.until(|| (0..6).all(|seq| kit.completed(ConcurrencyQueue::NAME, seq) == 1))
        .await;
    worker.stop().await;
    assert_eq!(
        probe::peak(ConcurrencyQueue::NAME),
        3,
        "three at once, and never four"
    );
}

/// A retryable failure with budget left runs again, as the job's next attempt,
/// and completes.
pub async fn a_retryable_failure_runs_again_as_the_next_attempt<B: KitBackend>(
    backend: B,
    logs: LogCapture,
) {
    let kit = Kit::begin(backend, logs, RetryQueue).await;
    let worker = kit.worker(WINDOW).await;
    let pushed = kit
        .push(&worker, RetryQueue, 0, Act::FailFirst { attempts: 1 }, None)
        .await;
    kit.until(|| kit.completed(RetryQueue::NAME, 0) == 1).await;
    worker.stop().await;
    let attempts = kit.attempts(RetryQueue::NAME, 0);
    assert_eq!(attempts.len(), 2, "the failure ran once more");
    assert_eq!(attempts[0].ended, Some(false));
    assert_eq!(
        kit.lines(&pushed)
            .iter()
            .filter_map(|line| line.field("attempt"))
            .collect::<Vec<_>>(),
        ["1", "2"],
        "both attempts at one job, numbered"
    );
}

/// A job failing every attempt is dead-lettered once its budget is spent —
/// `retries = 1`, two attempts — and runs no third.
pub async fn a_spent_budget_dead_letters_the_job<B: KitBackend>(backend: B, logs: LogCapture) {
    let kit = Kit::begin(backend, logs, BudgetQueue).await;
    let worker = kit.worker(WINDOW).await;
    let pushed = kit
        .push(&worker, BudgetQueue, 0, Act::FailAlways, None)
        .await;
    kit.until(|| {
        !kit.logs
            .find(TARGET, "job dead-lettered: retry budget spent")
            .is_empty()
    })
    .await;
    tokio::time::sleep(kit.backend.lease()).await;
    worker.stop().await;
    assert_eq!(
        kit.attempts(BudgetQueue::NAME, 0).len(),
        2,
        "no third attempt"
    );
    assert_eq!(
        kit.lines(&pushed).len(),
        2,
        "two attempts, each with its line"
    );
}

/// A worker killed between handing a job to its attempt and settling it leaves
/// the job to another worker once the lease lapses, which runs it — a job with
/// no retries included, since the delivery that died never answered.
pub async fn a_job_whose_worker_died_mid_attempt_runs_on_another<B: KitBackend>(
    backend: B,
    logs: LogCapture,
) {
    let kit = Kit::begin(backend, logs, DeathQueue).await;
    let first = kit.worker(WINDOW).await;
    let pushed = kit.push(&first, DeathQueue, 0, Act::ParkOnce, None).await;
    kit.until(|| kit.attempts(DeathQueue::NAME, 0).len() == 1)
        .await;
    first.kill().await;
    kit.backend
        .lapse(&queue_name::<DeathQueue>())
        .await
        .expect("the backend lapses the dead worker's lease");
    let second = kit.worker(WINDOW).await;
    kit.until(|| kit.completed(DeathQueue::NAME, 0) == 1).await;
    second.stop().await;
    assert_eq!(kit.attempts(DeathQueue::NAME, 0).len(), 2);
    assert_eq!(
        kit.lines(&pushed)
            .last()
            .and_then(|line| line.field("outcome")),
        Some(operation_log::OK.to_owned()),
        "completed, never dead-lettered",
    );
}

/// A lease taken from a worker still running its attempt — frozen past its
/// lease, then back — cuts that attempt: its outcome could not land, and the
/// worker holding the job now runs it and decides.
pub async fn a_lease_lost_under_a_live_worker_cuts_its_attempt_and_the_holder_decides<
    B: KitBackend,
>(
    backend: B,
    logs: LogCapture,
) {
    let kit = Kit::begin(backend, logs, TakenQueue).await;
    let first = kit.worker(WINDOW).await;
    let pushed = kit.push(&first, TakenQueue, 0, Act::ParkOnce, None).await;
    kit.until(|| kit.attempts(TakenQueue::NAME, 0).len() == 1)
        .await;
    let second = kit.worker(WINDOW).await;
    kit.backend
        .take(&queue_name::<TakenQueue>())
        .await
        .expect("the backend takes the lease");
    kit.until(|| kit.completed(TakenQueue::NAME, 0) == 1).await;
    kit.until(|| {
        kit.lines(&pushed)
            .iter()
            .any(|line| line.field("outcome").as_deref() == Some(operation_log::CANCELLED))
    })
    .await;
    first.stop().await;
    second.stop().await;
    assert_eq!(kit.attempts(TakenQueue::NAME, 0).len(), 2);
    assert!(
        !kit.logs
            .find(
                TARGET,
                "job lease taken by another delivery; its attempt is cut, and the delivery \
                 holding the job decides it",
            )
            .is_empty(),
        "the cut is said",
    );
}

/// A job whose every attempt takes its worker down — killed three times, never
/// answering — is dead-lettered on its fourth delivery without running.
pub async fn a_job_taking_its_worker_down_past_the_stall_limit_is_dead_lettered_unrun<
    B: KitBackend,
>(
    backend: B,
    logs: LogCapture,
) {
    let kit = Kit::begin(backend, logs, StallQueue).await;
    let mut worker = kit.worker(WINDOW).await;
    kit.push(&worker, StallQueue, 0, Act::ParkAlways, None)
        .await;
    for started in 1..=nest_rs_queue::STALL_LIMIT {
        let expected = usize::try_from(started).unwrap_or(usize::MAX);
        kit.until(|| kit.attempts(StallQueue::NAME, 0).len() == expected)
            .await;
        worker.kill().await;
        kit.backend
            .lapse(&queue_name::<StallQueue>())
            .await
            .expect("the backend lapses the dead worker's lease");
        worker = kit.worker(WINDOW).await;
    }
    kit.until(|| {
        !kit.logs
            .find(
                TARGET,
                "job dead-lettered: its deliveries ended without an answer past the stall limit",
            )
            .is_empty()
    })
    .await;
    worker.stop().await;
    assert_eq!(
        kit.attempts(StallQueue::NAME, 0).len(),
        usize::try_from(nest_rs_queue::STALL_LIMIT).unwrap_or(usize::MAX),
        "the delivery past the limit ran nothing",
    );
}

/// A stop lets an attempt that fits the window finish, cuts one that does not
/// and hands its job back, and the next worker runs it.
pub async fn the_drain_finishes_what_fits_its_window_and_hands_back_the_rest<B: KitBackend>(
    backend: B,
    logs: LogCapture,
) {
    let kit = Kit::begin(backend, logs, DrainQueue).await;
    let window = Duration::from_secs(1);
    let first = kit.worker(window).await;
    kit.push(&first, DrainQueue, 0, Act::Hold { ms: 200 }, None)
        .await;
    let parked = kit.push(&first, DrainQueue, 1, Act::ParkOnce, None).await;
    kit.until(|| {
        kit.attempts(DrainQueue::NAME, 0).len() == 1 && kit.attempts(DrainQueue::NAME, 1).len() == 1
    })
    .await;
    let stopping = Instant::now();
    first.stop().await;
    assert!(
        stopping.elapsed() < window + Duration::from_secs(1),
        "the stop stayed inside its window: {:?}",
        stopping.elapsed()
    );
    assert_eq!(
        kit.completed(DrainQueue::NAME, 0),
        1,
        "the short attempt finished"
    );
    assert!(
        kit.logs
            .find(TARGET, "job handed back to the queue")
            .iter()
            .any(|line| line.field("job_id") == Some(parked.id().to_string())),
        "the cut job was handed back",
    );
    let second = kit.worker(WINDOW).await;
    kit.until(|| kit.completed(DrainQueue::NAME, 1) == 1).await;
    second.stop().await;
    assert_eq!(kit.attempts(DrainQueue::NAME, 0).len(), 1);
    assert_eq!(kit.attempts(DrainQueue::NAME, 1).len(), 2);
}

/// An attempt running three leases long keeps its lease through renewals: a
/// second worker never takes it, and it runs once.
pub async fn a_long_attempt_keeps_its_lease_and_runs_once<B: KitBackend>(
    backend: B,
    logs: LogCapture,
) {
    let kit = Kit::begin(backend, logs, RenewalQueue).await;
    let hold = kit.backend.lease() * 3;
    let first = kit.worker(WINDOW).await;
    let second = kit.worker(WINDOW).await;
    let ms = u64::try_from(hold.as_millis()).unwrap_or(u64::MAX);
    kit.push(&first, RenewalQueue, 0, Act::Hold { ms }, None)
        .await;
    kit.until(|| kit.completed(RenewalQueue::NAME, 0) == 1)
        .await;
    first.stop().await;
    second.stop().await;
    assert_eq!(kit.attempts(RenewalQueue::NAME, 0).len(), 1, "it ran once");
    assert!(
        kit.logs.find(
            TARGET,
            "job lease not renewed for a whole lease; its attempt is cut and the job handed back",
        )
        .is_empty(),
        "no lease lapsed",
    );
}

/// A job pushed with a delay starts once the delay has passed, and not before —
/// on a backend declaring delayed delivery.
pub async fn a_delayed_push_runs_once_its_delay_has_passed<B: KitBackend>(
    backend: B,
    logs: LogCapture,
) {
    let kit = Kit::begin(backend, logs, DelayQueue).await;
    let worker = kit.worker(WINDOW).await;
    if !worker
        .producer
        .backend()
        .capabilities()
        .contains(Capability::DelayedPush)
    {
        worker.stop().await;
        return;
    }
    let delay = Duration::from_secs(1);
    let pushed_at = Instant::now();
    kit.push(
        &worker,
        DelayQueue,
        0,
        Act::Complete,
        Some(PushOptions::default().with_delay(delay)),
    )
    .await;
    kit.until(|| kit.completed(DelayQueue::NAME, 0) == 1).await;
    worker.stop().await;
    let started = kit.attempts(DelayQueue::NAME, 0)[0].started;
    // A backend timing the delay on its own clock may round to its
    // millisecond.
    assert!(
        started + Duration::from_millis(5) >= pushed_at + delay,
        "started {:?} after the push, before its delay",
        started.duration_since(pushed_at)
    );
}

/// A job runs in the trace its push was made in.
pub async fn a_job_runs_in_the_trace_that_pushed_it<B: KitBackend>(backend: B, logs: LogCapture) {
    let kit = Kit::begin(backend, logs, TraceQueue).await;
    let worker = kit.worker(WINDOW).await;
    let pushed_in = with_request_scope(None, Correlation::minted(None), async {
        kit.push(&worker, TraceQueue, 0, Act::Complete, None).await;
        current_trace_id().map(|trace| trace.to_string())
    })
    .await;
    kit.until(|| kit.completed(TraceQueue::NAME, 0) == 1).await;
    worker.stop().await;
    assert!(pushed_in.is_some());
    assert_eq!(kit.attempts(TraceQueue::NAME, 0)[0].trace, pushed_in);
}

/// One case's run against one backend, on the queue `C` marks.
struct Kit<B, C> {
    backend: B,
    logs: LogCapture,
    run: u64,
    case: PhantomData<fn() -> C>,
}

/// A worker of a case: its app, its transport's task, and the producer of the
/// same app.
struct Worker {
    serving: JoinHandle<anyhow::Result<()>>,
    stop: CancellationToken,
    producer: Arc<dyn JobProducer>,
    _app: HeadlessApp,
}

impl<B: KitBackend, C: Queue + CaseModule> Kit<B, C> {
    /// Start a case on `queue`'s marker, its storage emptied first.
    async fn begin(backend: B, logs: LogCapture, _queue: C) -> Self {
        backend
            .purge(&queue_name::<C>())
            .await
            .expect("the backend purges the case's queue");
        let run = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| u64::try_from(since.as_nanos()).unwrap_or(u64::MAX))
            .unwrap_or_default();
        Self {
            backend,
            logs,
            run,
            case: PhantomData,
        }
    }

    /// A worker over the backend, draining the case's queue within `window`.
    async fn worker(&self, window: Duration) -> Worker {
        let app = self
            .backend
            .app()
            .module::<C::Module>()
            .provide(QueueConfig {
                shutdown_timeout: window,
            })
            .build_headless()
            .await
            .expect("the kit's app boots over the backend");
        app.init().await.expect("init phases");
        let mut worker = QueueWorker::new();
        worker
            .configure(app.container())
            .await
            .expect("the queue worker configures over the backend");
        let stop = CancellationToken::new();
        let serving = tokio::spawn(Box::new(worker).serve(stop.clone()));
        let producer = app
            .container()
            .get_dyn::<dyn JobProducer>()
            .expect("the backend binds a producer");
        Worker {
            serving,
            stop,
            producer,
            _app: app,
        }
    }

    /// Push job `seq` of this run on `queue`, doing `act`.
    async fn push<Q: Destination<Job = KitCommand> + Send>(
        &self,
        worker: &Worker,
        queue: Q,
        seq: u32,
        act: Act,
        options: Option<PushOptions>,
    ) -> PushReceipt {
        worker
            .producer
            .push(
                queue,
                KitCommand {
                    run: self.run,
                    seq,
                    act,
                },
                options,
            )
            .await
            .expect("the push is filed")
    }

    fn attempts(&self, queue: &'static str, seq: u32) -> Vec<Attempt> {
        probe::attempts(queue, self.run, seq)
    }

    /// How many attempts at job `seq` on `queue` completed.
    fn completed(&self, queue: &'static str, seq: u32) -> usize {
        self.attempts(queue, seq)
            .iter()
            .filter(|attempt| attempt.ended == Some(true))
            .count()
    }

    /// The operation lines of the job `pushed` names, in the order filed.
    fn lines(&self, pushed: &PushReceipt) -> Vec<crate::CapturedEvent> {
        let job = pushed.id().to_string();
        self.logs
            .find(operation_log::TARGET, unit::JOB.name())
            .into_iter()
            .filter(|line| line.field("job_id").as_deref() == Some(job.as_str()))
            .collect()
    }

    /// Wait for `done`, within a few leases and [`PATIENCE`].
    #[track_caller]
    fn until(&self, done: impl FnMut() -> bool) -> impl Future<Output = ()> {
        crate::wait_until(self.backend.lease() * 4 + PATIENCE, done)
    }
}

impl Worker {
    /// Stop the worker as a shutdown signal does, and wait for its drain.
    async fn stop(self) {
        self.stop.cancel();
        self.serving
            .await
            .expect("the worker's task ends")
            .expect("the worker stops cleanly");
    }

    /// Kill the worker where it stands, as a killed process dies: nothing it
    /// runs settles.
    async fn kill(self) {
        self.serving.abort();
        let _ended = self.serving.await;
    }
}

/// The name of `Q`'s queue, checked.
fn queue_name<Q: Queue>() -> QueueName {
    QueueName::new(Q::NAME).expect("the kit's queue names are valid")
}
