//! apalis-redis `JobConsumer` exposed as a `Transport`: one apalis worker per
//! discovered `#[process]` method on a shared [`Monitor`].
//!
//! **This file is the transport and nothing else.** What a job attempt *is* —
//! the envelope, the trace, the `queue.job` span, the panic catch, the outcome
//! classes, the retry budget, the events and the operation line — is the port's
//! (`nest_rs_queue::consume::attempt`), and discovery is the port's too
//! (`consume::discover`, which refuses every declaration this backend does not
//! honour). What stays here is what apalis alone knows: the storage handle, the
//! fetch loop, a method's `concurrency`, the drain at shutdown, and how the
//! port's [`AttemptOutcome`] settles an apalis task.
//!
//! Every queue is consumed as `RedisStorage<serde_json::Value>` — the
//! backend-agnostic wire format.
//!
//! **Concurrency is per method, per replica.** `#[process(concurrency = N)]`
//! bounds how many attempts of that method one worker replica runs at once
//! (default 1); throughput beyond it comes from running more replicas, which the
//! platform schedules and meters. Each method is its own apalis worker, so one
//! method's jobs never wait on another's permits.
//!
//! **apalis never retries on its own.** The budget is the port's: every attempt
//! a delivery gets runs inside the one apalis task that fetched it, and the task
//! settles once — done, or dead-lettered through apalis's `Abort`, which its
//! acknowledgement kills rather than re-queues. A plain error would hand the job
//! to apalis's own retry count instead, which knows nothing of the method's — so
//! the one place this file answers with one is a shutdown Redis would not let
//! hand the job back, where the job still being in flight, and so running again,
//! is the point.
//!
//! **A retry waits here, in process.** This backend does not declare delayed
//! delivery, so the wait the port names before a job's next attempt
//! ([`AttemptOutcome::Retry`]) passes inside the delivery, holding the method's
//! permit — a method with `concurrency = 1` runs nothing else on this replica
//! while one of its jobs waits. A shutdown ends the wait, never the job: the
//! apalis task is rescheduled in Redis for when the wait would have ended,
//! carrying the record the port hands back for the next attempt
//! ([`Delivery::retry_envelope`]) — so the job keeps its id, its trace and its
//! attempt count on whichever replica takes it next.
//!
//! **The hand-back never waits on an acknowledgement.** apalis-redis 0.7
//! acknowledges a task through a channel its worker's heartbeat drains, and the
//! heartbeat is dropped the moment the last task of a stopping worker ends — so
//! the acknowledgement of a task finishing during the drain never reaches Redis,
//! the task stays in the worker's in-flight set, and the next replica to start
//! runs it again. A hand-back that relied on it would run the job twice: once
//! from the stored record, once from the schedule. So the task is moved out of
//! flight by the hand-back itself, through apalis's own `reschedule`, before the
//! delivery ends.
//!
//! **Delivery is exclusive but at-least-once.** Two replicas never receive the
//! same job from the queue (`get_jobs.lua` claims ids in one atomic EVAL), yet a
//! replica's *startup* requeues work its peers are running: apalis-redis calls
//! `reenqueue_orphaned` with a cutoff of `Utc::now()`, which matches every
//! registered consumer rather than only this worker's previous incarnation. A
//! scale-up therefore re-runs in-flight jobs. Both halves are measured in
//! `tests/e2e/replicas.rs`; a `#[process]` handler must be idempotent.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use apalis::layers::ErrorHandlingLayer;
use apalis::layers::WorkerBuilderExt;
use apalis::layers::catch_panic::CatchPanicLayer;
use apalis::prelude::{
    Attempt, Data, Monitor, Request, Storage, TaskId, WorkerBuilder, WorkerFactoryFn,
};
use apalis_redis::{Config, RedisContext, RedisStorage};
use async_trait::async_trait;
use nest_rs_core::{Container, Transport};
use nest_rs_queue::consume::{self, AttemptOutcome, Delivery};
use nest_rs_queue::{JobError, ProcessMethod, QueueName};
use tokio_util::sync::CancellationToken;

use crate::RedisConnection;
use crate::backend::BACKEND;
use crate::connection::CONNECTION_REMEDY;

/// How often a worker moves the records whose time has come from Redis's
/// scheduled set to its queue — see `build_worker`.
const SCHEDULED_SCAN: Duration = Duration::from_secs(1);

/// The consumer-side transport: drains the `#[processor]` inventory and runs
/// each job's process method against the Redis queue. Attached by
/// [`RedisWorkerModule`](crate::RedisWorkerModule).
pub struct RedisWorker {
    methods: Vec<&'static ProcessMethod>,
    container: Option<Container>,
}

impl RedisWorker {
    /// An empty worker; process methods and the container are wired at boot.
    pub fn new() -> Self {
        Self {
            methods: Vec::new(),
            container: None,
        }
    }
}

impl Default for RedisWorker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Transport for RedisWorker {
    async fn configure(&mut self, container: &Container) -> Result<()> {
        // Which `#[process]` methods this app serves is the port's answer —
        // module-gated, duplicate-checked, refused where this backend lacks a
        // capability a method declares, announced — not this backend's.
        self.methods = consume::discover(container, &BACKEND)?;

        // Fail fast at boot if methods exist but no connection is seeded.
        if !self.methods.is_empty() {
            container.get::<RedisConnection>().with_context(|| {
                format!("RedisWorker found #[processor]s but {CONNECTION_REMEDY}")
            })?;
        }

        self.container = Some(container.clone());
        Ok(())
    }

    async fn serve(self: Box<Self>, cancel: CancellationToken) -> Result<()> {
        // No methods: idle until shutdown so this transport doesn't race
        // the app down when it is the only one attached.
        if self.methods.is_empty() {
            cancel.cancelled().await;
            return Ok(());
        }

        let container = self
            .container
            .expect("RedisWorker::configure must run before serve");
        let connection = container
            .get::<RedisConnection>()
            .expect("RedisConnection presence is verified in configure");

        let mut monitor = Monitor::new();
        for method in &self.methods {
            monitor = build_worker(monitor, &connection, container.clone(), method, &cancel)?;
        }

        // Bound the post-signal drain so a hung `#[process]` can't block SIGTERM
        // until the orchestrator SIGKILLs the pod (QUEUE-I5). The config is a
        // factory output `RedisWorkerModule::for_root` resolved.
        let shutdown_timeout = container
            .get::<crate::RedisWorkerConfig>()
            .map(|cfg| cfg.shutdown_timeout)
            .unwrap_or_else(|| crate::RedisWorkerConfig::default().shutdown_timeout);

        monitor
            .shutdown_timeout(shutdown_timeout)
            .run_with_signal(async move {
                cancel.cancelled().await;
                Ok(())
            })
            .await?;
        Ok(())
    }
}

/// Build one apalis worker for a `ProcessMethod`. The wire payload is always
/// `serde_json::Value`; the port's `attempt` opens the envelope and the
/// macro-emitted handler deserializes it to the method's job type, so this
/// builder never names it.
fn build_worker(
    monitor: Monitor,
    conn: &RedisConnection,
    container: Container,
    method: &'static ProcessMethod,
    shutdown: &CancellationToken,
) -> Result<Monitor> {
    // Checked by `discover` already, which refuses a name outside the rule and
    // every dynamic queue — this backend declares none — so this is the static
    // name the method drains, parsed once per worker rather than per job.
    let queue = QueueName::new(method.queue())?;
    // Fetch one job per poll. apalis 0.7 drives a fetched batch through a
    // `FuturesUnordered` and keeps polling while those futures are in flight, so
    // the buffer alone bounds nothing — it is `concurrency` below that bounds the
    // work. Sizing the buffer at one matters anyway: a job sitting in a saturated
    // worker's buffer is invisible to every other replica, which is exactly the
    // throughput the deployment is paying for. Namespaced under the queue name,
    // which is how apalis routes a producer's job to this worker.
    //
    // A record scheduled for later — a job handed back at shutdown — becomes
    // available on apalis's `enqueue_scheduled` heartbeat, which sleeps before its
    // first tick and defaults to thirty seconds: that would run a job's next
    // attempt up to half a minute after the wait the port asked for, which at a
    // one-second backoff is thirty times it. A second keeps the lateness under the
    // port's own jitter, for one scheduled-set scan per method per second.
    let storage: RedisStorage<serde_json::Value, RedisConnection> = RedisStorage::new_with_config(
        conn.clone(),
        Config::default()
            .set_namespace(method.queue())
            .set_buffer_size(1)
            .set_enqueue_scheduled(SCHEDULED_SCAN),
    );
    // The handle a delivery hands its job back through at shutdown — the same
    // namespace, so the next attempt lands on this queue.
    let hand_back_to = storage.clone();
    let shutdown = shutdown.clone();
    // Position is load-bearing for the panic layer: `consume::attempt` catches a
    // handler panic itself (so the event lands inside the per-job span), which
    // leaves this layer as the **backstop** for a panic outside that call — in
    // apalis's own fetch/deserialize path, or in the closure prologue. It turns
    // one into apalis's `Abort`, which dead-letters rather than re-queues, so one
    // bad job cannot take down the queue's consumer.
    let worker = WorkerBuilder::new(method.queue())
        // The method's permits, and the outermost layer so a permit covers a
        // job's whole delivery — every attempt of its budget included. apalis
        // delegates `poll_ready` to the inner service, so with every permit held
        // the fetch loop backs off rather than piling work into memory: the next
        // job stays in Redis, where another replica can take it.
        .concurrency(method.options().concurrency().get() as usize)
        .layer(ErrorHandlingLayer::new())
        .layer(CatchPanicLayer::new())
        .data(container)
        .backend(storage)
        .build_fn(
            move |job: serde_json::Value,
                  container: Data<Container>,
                  task_id: TaskId,
                  attempt: Attempt,
                  context: RedisContext| {
                // apalis's task id is its record's, never the job's: the job's
                // id is the one the port sealed, and this rides beside it.
                let delivery = Delivery::new(&BACKEND, queue.clone(), job)
                    .with_backend_id(task_id.to_string());
                let task = Task {
                    id: task_id,
                    attempt,
                    context,
                };
                let hand_back_to = hand_back_to.clone();
                let shutdown = shutdown.clone();
                async move {
                    deliver(
                        method,
                        delivery,
                        task,
                        (*container).clone(),
                        hand_back_to,
                        shutdown,
                    )
                    .await
                }
            },
        );
    Ok(monitor.register(worker))
}

/// The error type apalis's `build_fn` closure returns.
type BoxDynError = Box<dyn std::error::Error + Send + Sync>;

/// The apalis task a delivery arrived as — what a hand-back rewrites in place.
struct Task {
    id: TaskId,
    attempt: Attempt,
    context: RedisContext,
}

impl Task {
    /// The task, carrying `record` in place of what it was fetched with.
    fn carrying(&self, record: serde_json::Value) -> Request<serde_json::Value, RedisContext> {
        let mut request = Request::new_with_ctx(record, self.context.clone());
        request.parts.task_id = self.id.clone();
        request.parts.attempt = self.attempt.clone();
        request
    }
}

/// Run the port's attempts at one fetched job until the port settles it, and
/// answer apalis once: `Ok` when it completed or was handed back at shutdown,
/// [`dead_letter`] when it did not complete, and a plain failure when a shutdown
/// could not hand it back ([`handed_back`] says why).
async fn deliver(
    method: &'static ProcessMethod,
    mut delivery: Delivery,
    task: Task,
    container: Container,
    mut hand_back_to: RedisStorage<serde_json::Value, RedisConnection>,
    shutdown: CancellationToken,
) -> Result<(), BoxDynError> {
    loop {
        match consume::attempt(method, &mut delivery, container.clone()).await {
            AttemptOutcome::Ok => return Ok(()),
            AttemptOutcome::DeadLetter(error) => return Err(dead_letter(error)),
            AttemptOutcome::Retry { after } => {
                let due = SystemTime::now() + after;
                tokio::select! {
                    () = tokio::time::sleep(after) => {}
                    () = shutdown.cancelled() => {
                        return hand_back(&mut hand_back_to, &delivery, &task, due).await;
                    }
                }
            }
        }
    }
}

/// Reschedule the task for `due`, carrying the record the port hands back for
/// the job's next attempt, so a shutdown gives up the wait and never the job.
///
/// **Onto the schedule first, then out of flight** — two calls, in that order,
/// so no failure between them loses the job. apalis's `reschedule` removes the
/// task from the worker's in-flight set *before* it schedules it, as separate
/// commands, and a failure between the two would leave a job in neither place;
/// scheduling the same task first means every failure after it leaves the job
/// scheduled, at worst still in flight as well.
async fn hand_back(
    storage: &mut RedisStorage<serde_json::Value, RedisConnection>,
    delivery: &Delivery,
    task: &Task,
    due: SystemTime,
) -> Result<(), BoxDynError> {
    let at = schedule_at(due);
    let record = || task.carrying(delivery.retry_envelope().into_json());
    let outcome = match storage.schedule_request(record(), at).await {
        Err(error) => HandBack::NotScheduled(error),
        Ok(_) => match storage.reschedule(record(), wait_until(at)).await {
            Ok(()) => HandBack::Whole,
            Err(error) => HandBack::StillInFlight(error),
        },
    };
    handed_back(delivery, outcome)
}

/// `due` as the whole second apalis schedules on — rounded up, so the wait is at
/// least as long as the port asked for.
fn schedule_at(due: SystemTime) -> i64 {
    let on = due
        .duration_since(UNIX_EPOCH)
        .map(|since| {
            since
                .as_secs()
                .saturating_add(u64::from(since.subsec_nanos() > 0))
        })
        .unwrap_or_default();
    i64::try_from(on).unwrap_or(i64::MAX)
}

/// The wait `reschedule` takes to land on the second `at`: it counts whole
/// seconds from the current one, so a second that ticks over before it reads the
/// clock only ever makes the wait longer.
fn wait_until(at: i64) -> Duration {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    let now = i64::try_from(now).unwrap_or(i64::MAX);
    Duration::from_secs(u64::try_from(at.saturating_sub(now)).unwrap_or_default())
}

/// How far a hand-back got.
enum HandBack<E> {
    /// Scheduled for the next attempt and out of flight.
    Whole,
    /// Redis refused the schedule, so nothing changed: the task is still in
    /// flight under the record it was fetched with.
    NotScheduled(E),
    /// Scheduled for the next attempt, and Redis refused to take it out of
    /// flight.
    StillInFlight(E),
}

/// What a hand-back answers apalis once Redis has answered, and the line saying
/// which way it went.
///
/// A task Redis would not schedule is failed rather than acknowledged: it is
/// still in flight, so the next replica to start runs it again from the attempt
/// it was stored at — a budget restarted rather than a job lost. Acknowledging
/// it would drop the job, and dead-lettering it would bury a job over a failure
/// that was never its own. A task scheduled and still in flight is
/// acknowledged: when the acknowledgement lands it takes the task out of flight,
/// and when it does not, the next replica to start runs the next attempt early
/// and the schedule runs it again — twice, which the line says, rather than
/// never.
fn handed_back<E>(delivery: &Delivery, outcome: HandBack<E>) -> Result<(), BoxDynError>
where
    E: std::error::Error + Send + Sync + 'static,
{
    match outcome {
        HandBack::Whole => {
            tracing::info!(
                target: nest_rs_queue::TARGET,
                queue = %delivery.queue(),
                job_id = %delivery.id(),
                attempt = delivery.attempt(),
                "job handed back at shutdown for its next attempt",
            );
            Ok(())
        }
        HandBack::NotScheduled(error) => {
            tracing::error!(
                target: nest_rs_queue::TARGET,
                queue = %delivery.queue(),
                job_id = %delivery.id(),
                error = %nest_rs_core::error_message(&error),
                "job not handed back at shutdown; it runs again from its stored attempt",
            );
            Err(Box::new(error))
        }
        HandBack::StillInFlight(error) => {
            tracing::warn!(
                target: nest_rs_queue::TARGET,
                queue = %delivery.queue(),
                job_id = %delivery.id(),
                attempt = delivery.attempt(),
                error = %nest_rs_core::error_message(&error),
                "job handed back at shutdown and still in flight; its next attempt may run twice",
            );
            Ok(())
        }
    }
}

/// A job the port dead-lettered, as apalis's `Abort`: its acknowledgement kills
/// the task onto the dead set at once. A plain error would read as apalis's
/// `Failed`, which its acknowledgement re-queues under apalis's own attempt
/// count — a second budget, invisible to the method's.
fn dead_letter(error: JobError) -> BoxDynError {
    Box::new(apalis::prelude::Error::Abort(Arc::new(error.source)))
}

#[cfg(test)]
mod tests {
    use nest_rs_testing::LogCapture;

    use super::*;

    /// A job waiting out the backoff before its second attempt.
    fn delivery() -> Delivery {
        Delivery::new(
            &BACKEND,
            QueueName::new("audio").expect("a valid name"),
            serde_json::json!({
                "v": nest_rs_queue::WIRE_FORMAT_VERSION,
                "id": "01890a5d-ac96-774b-bcce-b302099a8057",
                "attempt": 2,
                "payload": { "clip": 1 },
            }),
        )
    }

    /// The whole hand-back is said once, naming the attempt it hands back.
    #[test]
    fn a_whole_hand_back_is_acknowledged_and_names_the_next_attempt() {
        let logs = LogCapture::install();
        let delivery = delivery();
        handed_back::<std::io::Error>(&delivery, HandBack::Whole).expect("acknowledged");

        let event = logs.expect_one(
            nest_rs_queue::TARGET,
            "job handed back at shutdown for its next attempt",
        );
        assert_eq!(
            event.field("job_id").as_deref(),
            Some("01890a5d-ac96-774b-bcce-b302099a8057"),
        );
        assert_eq!(event.field("attempt").as_deref(), Some("2"));
    }

    /// A hand-back Redis refused must neither drop the job nor bury it: the task
    /// fails as a plain error, which apalis re-queues, and the line says the job
    /// runs again rather than that it was handed back.
    #[test]
    fn a_hand_back_redis_refuses_fails_the_task_and_says_the_job_runs_again() {
        let logs = LogCapture::install();
        let delivery = delivery();
        let failed = handed_back(
            &delivery,
            HandBack::NotScheduled(std::io::Error::other("connection refused")),
        )
        .expect_err("the task is failed, never acknowledged");
        assert!(
            !failed
                .downcast_ref::<apalis::prelude::Error>()
                .is_some_and(|e| matches!(e, apalis::prelude::Error::Abort(_))),
            "a failed hand-back is not a dead letter: {failed}",
        );

        let event = logs.expect_one(
            nest_rs_queue::TARGET,
            "job not handed back at shutdown; it runs again from its stored attempt",
        );
        assert_eq!(event.level, "error");
        assert_eq!(event.field("job_id"), Some(delivery.id().to_string()));
        assert_eq!(event.field("queue").as_deref(), Some("audio"));
        assert_eq!(event.field("error").as_deref(), Some("connection refused"));
        logs.expect_none(
            nest_rs_queue::TARGET,
            "job handed back at shutdown for its next attempt",
        );
    }

    /// Scheduled but left in flight: the job's next attempt is safe, and may run
    /// twice — which is said at `warn`, since it is a duplicate and not a loss.
    #[test]
    fn a_hand_back_left_in_flight_is_acknowledged_and_says_the_attempt_may_run_twice() {
        let logs = LogCapture::install();
        let delivery = delivery();
        handed_back(
            &delivery,
            HandBack::StillInFlight(std::io::Error::other("connection reset")),
        )
        .expect("acknowledged, so a landing acknowledgement takes it out of flight");

        let event = logs.expect_one(
            nest_rs_queue::TARGET,
            "job handed back at shutdown and still in flight; its next attempt may run twice",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("job_id"), Some(delivery.id().to_string()));
        assert_eq!(event.field("error").as_deref(), Some("connection reset"));
    }

    /// apalis schedules on whole seconds, and a wait shortened by the rounding
    /// would run the next attempt before the backoff the port asked for.
    #[test]
    fn a_hand_back_is_scheduled_on_the_second_the_wait_ends_or_after_it() {
        let at = |millis: u64| UNIX_EPOCH + Duration::from_millis(millis);
        assert_eq!(schedule_at(at(10_000)), 10);
        assert_eq!(schedule_at(at(10_001)), 11);
        assert_eq!(schedule_at(at(10_999)), 11);

        let now = schedule_at(SystemTime::now());
        assert!(wait_until(now + 5) >= Duration::from_secs(5));
        assert_eq!(
            wait_until(0),
            Duration::ZERO,
            "a second already past waits for nothing"
        );
    }

    /// The mapping is one line and nothing else exercises it in process: a plain
    /// error in its place would pass every suite here and hand a dead letter to
    /// apalis's own retry count.
    #[test]
    fn a_dead_letter_is_apalis_abort_which_never_re_queues() {
        let dead = dead_letter(JobError::abort("bad payload"));
        assert!(
            dead.downcast_ref::<apalis::prelude::Error>()
                .is_some_and(|e| matches!(e, apalis::prelude::Error::Abort(_))),
            "a dead letter is apalis's Abort, which its acknowledgement kills: {dead}",
        );
        assert!(
            dead.to_string().contains("bad payload"),
            "and it keeps the port's sentence for the dead set: {dead}",
        );
    }
}
