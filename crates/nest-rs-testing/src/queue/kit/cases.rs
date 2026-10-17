//! The kit's cases: each pushes jobs through the backend's producer, runs them
//! on the port's [`QueueWorker`] over the backend's consumer, and asserts what
//! the contract promises — read off what the jobs' attempts reported and the
//! lines the port filed.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nest_rs_core::{Correlation, Transport, current_trace_id, operation_log, with_request_scope};
use nest_rs_queue::{
    Capability, Destination, Envelope, JobProducer, JobProducerExt, PushOptions, PushReceipt,
    Queue, QueueBackend, QueueConfig, QueueError, QueueName, QueueWorker, TARGET, async_trait,
    unit,
};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::KitBackend;
use super::command::{
    Act, BudgetQueue, ConcurrencyQueue, DeathQueue, DelayQueue, DrainQueue, KitCommand, OnceQueue,
    RenewalQueue, ResendQueue, RetryQueue, StallQueue, TakenQueue, TraceQueue,
};
use super::module::CaseModule;
use super::probe::{self, Attempt};
use crate::{HeadlessApp, LogCapture};

/// The drain window of a case's workers.
const WINDOW: Duration = Duration::from_secs(2);

/// The longest a case waits for what it expects, past the leases it waits out.
const PATIENCE: Duration = Duration::from_secs(20);

/// The latest a first retry falls due after its failure: the port's one second,
/// jittered up to a fifth longer.
const FIRST_RETRY_AT_LATEST: Duration = Duration::from_millis(1200);

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

/// An envelope enqueued again while the backend holds its job — waiting, held
/// back by a delay or a retry, running — is answered `Ok` and filed nothing, so
/// the job runs once; enqueued again once the job ended, it is filed and runs
/// again. Under a unique key, the envelope sent again is never refused as its
/// own key's holder, and its job keeps the key.
pub async fn an_envelope_enqueued_again_while_its_job_is_held_is_filed_once<B: KitBackend>(
    backend: B,
    logs: LogCapture,
) {
    let kit = Kit::begin(backend, logs, ResendQueue).await;
    let queue = queue_name::<ResendQueue>();
    let pushing = kit.producer().await;
    let recording = Recording::new(Arc::clone(&pushing.producer));
    let capabilities = recording.backend().capabilities();
    kit.push_on(&recording, ResendQueue, 0, Act::Hold { ms: 300 }, None)
        .await;
    let pushed = recording.sent();
    let delayed = if capabilities.contains(Capability::DelayedPush) {
        let options = PushOptions::default().with_delay(Duration::from_secs(1));
        kit.push_on(
            &recording,
            ResendQueue,
            1,
            Act::Complete,
            Some(options.clone()),
        )
        .await;
        Some((recording.sent(), options))
    } else {
        None
    };
    kit.push_on(
        &recording,
        ResendQueue,
        2,
        Act::FailFirst { attempts: 1 },
        None,
    )
    .await;
    let retried = recording.sent();
    let keyed = if capabilities.contains(Capability::UniquePush) {
        let options = PushOptions::default().with_unique(format!("kit-resend-{}", kit.run));
        let holder = kit
            .push_on(
                &recording,
                ResendQueue,
                3,
                Act::Complete,
                Some(options.clone()),
            )
            .await;
        Some((recording.sent(), options, holder))
    } else {
        None
    };
    enqueue_again(&pushing, &queue, &pushed, None, "waiting").await;
    if let Some((envelopes, options)) = &delayed {
        enqueue_again(&pushing, &queue, envelopes, Some(options), "held back").await;
    }
    if let Some((envelopes, options, holder)) = &keyed {
        enqueue_again(&pushing, &queue, envelopes, Some(options), "keyed").await;
        let refused = pushing
            .producer
            .push(
                ResendQueue,
                KitCommand {
                    run: kit.run,
                    seq: 4,
                    act: Act::Complete,
                },
                Some(options.clone()),
            )
            .await;
        assert!(
            matches!(
                &refused,
                Err(QueueError::UniqueKeyHeld { holder: held_by, .. }) if held_by == holder.id()
            ),
            "the key stays with the job sent again: {refused:?}"
        );
    }
    let first = kit.worker(WINDOW).await;
    kit.until(|| !kit.attempts(ResendQueue::NAME, 0).is_empty())
        .await;
    enqueue_again(&pushing, &queue, &pushed, None, "running").await;
    kit.until(|| {
        kit.attempts(ResendQueue::NAME, 2)
            .first()
            .is_some_and(|attempt| attempt.ended == Some(false))
    })
    .await;
    let retry_due = Instant::now() + FIRST_RETRY_AT_LATEST;
    enqueue_again(&pushing, &queue, &retried, None, "held back by a retry").await;
    assert_eq!(
        kit.attempts(ResendQueue::NAME, 2).len(),
        1,
        "sent again while its retry waits its backoff"
    );
    kit.until(|| {
        kit.completed(ResendQueue::NAME, 0) >= 1
            && (delayed.is_none() || kit.completed(ResendQueue::NAME, 1) >= 1)
            && kit.completed(ResendQueue::NAME, 2) >= 1
            && (keyed.is_none() || kit.completed(ResendQueue::NAME, 3) >= 1)
    })
    .await;
    tokio::time::sleep_until(retry_due).await;
    tokio::time::sleep(kit.backend.lease()).await;
    first.stop().await;
    assert_eq!(
        kit.attempts(ResendQueue::NAME, 0).len(),
        1,
        "sent three times while held, run once"
    );
    if delayed.is_some() {
        assert_eq!(
            kit.attempts(ResendQueue::NAME, 1).len(),
            1,
            "sent twice while held back, run once"
        );
    }
    let retries: Vec<_> = kit
        .attempts(ResendQueue::NAME, 2)
        .iter()
        .map(|attempt| attempt.ended)
        .collect();
    assert_eq!(
        retries,
        [Some(false), Some(true)],
        "sent again while its retry waits, failed once and completed once"
    );
    if keyed.is_some() {
        assert_eq!(
            kit.attempts(ResendQueue::NAME, 3).len(),
            1,
            "sent twice under its unique key, run once"
        );
        assert!(
            kit.attempts(ResendQueue::NAME, 4).is_empty(),
            "the push refused under the held key filed nothing"
        );
    }

    enqueue_again(&pushing, &queue, &pushed, None, "whose job ended").await;
    let second = kit.worker(WINDOW).await;
    kit.until(|| kit.completed(ResendQueue::NAME, 0) == 2).await;
    second.stop().await;
    assert_eq!(
        kit.attempts(ResendQueue::NAME, 0).len(),
        2,
        "sent again once its job ended, run again"
    );
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

/// An app of a case pushing through the backend, with no worker draining.
struct Pushing {
    producer: Arc<dyn JobProducer>,
    _app: HeadlessApp,
}

/// A producer handing every call to the backend's, keeping a copy of the
/// envelopes the port sealed — what the port alone can make, and what a case
/// enqueues again.
struct Recording {
    inner: Arc<dyn JobProducer>,
    sent: Mutex<Vec<Envelope>>,
}

impl Recording {
    fn new(inner: Arc<dyn JobProducer>) -> Self {
        Self {
            inner,
            sent: Mutex::default(),
        }
    }

    /// The envelopes handed over since the last call, in order.
    fn sent(&self) -> Vec<Envelope> {
        std::mem::take(&mut *self.sent.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

#[async_trait]
impl JobProducer for Recording {
    fn backend(&self) -> &'static QueueBackend {
        self.inner.backend()
    }

    async fn enqueue(
        &self,
        queue: &QueueName,
        envelopes: Vec<Envelope>,
        options: &PushOptions,
    ) -> Result<(), QueueError> {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(envelopes.iter().cloned());
        self.inner.enqueue(queue, envelopes, options).await
    }
}

/// Hand `envelopes` to the backend once more, as a re-sent call does, and
/// expect `Ok` for a job `state`.
async fn enqueue_again(
    pushing: &Pushing,
    queue: &QueueName,
    envelopes: &[Envelope],
    options: Option<&PushOptions>,
    state: &str,
) {
    pushing
        .producer
        .enqueue(
            queue,
            envelopes.to_vec(),
            options.unwrap_or(&PushOptions::default()),
        )
        .await
        .unwrap_or_else(|error| panic!("an envelope whose job is {state} is answered Ok: {error}"));
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

    /// An app over the backend, booted and initialised, draining nothing yet.
    async fn app(&self, window: Duration) -> HeadlessApp {
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
        app
    }

    /// An app pushing through the backend, with no worker.
    async fn producer(&self) -> Pushing {
        let app = self.app(WINDOW).await;
        Pushing {
            producer: producer_of(&app),
            _app: app,
        }
    }

    /// A worker over the backend, draining the case's queue within `window`.
    async fn worker(&self, window: Duration) -> Worker {
        let app = self.app(window).await;
        let mut worker = QueueWorker::new();
        worker
            .configure(app.container())
            .await
            .expect("the queue worker configures over the backend");
        let stop = CancellationToken::new();
        let serving = tokio::spawn(Box::new(worker).serve(stop.clone()));
        Worker {
            serving,
            stop,
            producer: producer_of(&app),
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
        self.push_on(&*worker.producer, queue, seq, act, options)
            .await
    }

    /// Push job `seq` of this run on `queue` through `producer`, doing `act`.
    async fn push_on<Q: Destination<Job = KitCommand> + Send>(
        &self,
        producer: &dyn JobProducer,
        queue: Q,
        seq: u32,
        act: Act,
        options: Option<PushOptions>,
    ) -> PushReceipt {
        producer
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

/// The producer `app`'s backend binds.
fn producer_of(app: &HeadlessApp) -> Arc<dyn JobProducer> {
    app.container()
        .get_dyn::<dyn JobProducer>()
        .expect("the backend binds a producer")
}

/// The name of `Q`'s queue, checked.
fn queue_name<Q: Queue>() -> QueueName {
    QueueName::new(Q::NAME).expect("the kit's queue names are valid")
}
