//! [`Deliveries`] — one delivery of a job, from the moment apalis hands it over
//! to the one answer apalis gets back.
//!
//! **A delivery runs on a task of its own.** apalis drives every job a worker
//! fetched, its heartbeat and its fetch loop inside one future; a delivery
//! spawned beside it cannot take that future down with a panic, and a handler
//! that blocks its thread blocks that thread only — the heartbeat that keeps the
//! replica alive in its peers' eyes, and the lease renewal that keeps the job
//! its own, run elsewhere. The task is aborted when apalis drops the delivery,
//! so a worker that stops holds no job running behind its back.
//!
//! **Then the guard** ([`Leases`]): the job runs only under its lease; a job
//! that already reached its outcome, or that a cancel reached first, is
//! acknowledged without running; a job whose lease another delivery holds is
//! handed back for when that lease would lapse, and one its method's throttle
//! holds back for when the window ends — as it was fetched, the attempt it is at
//! neither counted nor spent.
//!
//! **Then the port's attempt** ([`consume::attempt`]), and its outcome settles
//! the job once:
//!
//! | outcome | settled mark | answer to apalis |
//! | --- | --- | --- |
//! | `Ok` | `completed` | acknowledged |
//! | `DeadLetter` | `dead-lettered` | apalis's `Abort`, which kills it onto the dead set |
//! | `Retry { after }` | none, the lease dropped | the next attempt filed for `after`, and the task out of flight |
//!
//! A method taking a `Checkpoint` reads and saves it through [`RedisCheckpoint`],
//! keyed by the job's id, so every delivery of the job resumes from the last
//! save.
//!
//! apalis never retries on its own. The budget is the port's; a dead letter is
//! apalis's `Abort` rather than a plain error, which apalis would re-queue under
//! a count of its own, invisible to the method's. The one place a delivery
//! answers with a plain error is a hand-back Redis refused, where apalis filing
//! the stored record back — and so the job running again — is the point. apalis would kill instead once its own count of the record's
//! deliveries reached its cap, so every record this crate files lifts the cap
//! past any count (`uncapped_context`): hand-backs, deferrals and retries
//! never add up to an ending the port did not decide.
//!
//! **A hand-back never waits on an acknowledgement.** apalis-redis 0.7
//! acknowledges a task through a channel its worker's heartbeat drains, and the
//! heartbeat is dropped the moment the last task of a stopping worker ends — so
//! the acknowledgement of a task ending during the drain never reaches Redis.
//! A hand-back therefore takes the task out of flight itself, through apalis's
//! own `reschedule`; and a job settled while the worker drains keeps its settled
//! mark for [`SETTLED_WHILE_DRAINING`], since whichever replica starts next
//! delivers it again, whenever that is.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apalis::prelude::{Attempt, Request, Storage, TaskId};
use apalis_redis::{RedisContext, RedisStorage};
use nest_rs_core::{Container, panic_message};
use nest_rs_queue::consume::{self, AttemptOutcome, Delivery};
use nest_rs_queue::{Envelope, JobError, JobId, ProcessMethod, QueueName};
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

use super::checkpoint::RedisCheckpoint;
use super::lease::{Admission, Lease, Leases, Settlement};
use crate::RedisConnection;
use crate::backend::{BACKEND, due_second};

/// How long the settled mark of a job settled during a drain is kept: a week.
/// Its acknowledgement may never reach Redis, and the job is then delivered again
/// by whichever replica starts next — after a weekend scaled to zero, say — so
/// the mark has to outlive the quiet, not only a sweep.
pub(crate) const SETTLED_WHILE_DRAINING: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The shortest a job is handed back for — apalis schedules on whole seconds.
const SHORTEST_HAND_BACK: Duration = Duration::from_secs(1);

/// The error type apalis's `build_fn` closure returns.
pub(crate) type BoxDynError = Box<dyn std::error::Error + Send + Sync>;

/// What every delivery of one `#[process]` method shares: the method, its queue,
/// its connection and storage, its guard, the container its attempts resolve
/// from, and the two signals of a shutdown.
pub(crate) struct Deliveries {
    pub(crate) method: &'static ProcessMethod,
    pub(crate) queue: QueueName,
    /// The shared connection, which a method's checkpoints are kept over.
    pub(crate) conn: RedisConnection,
    /// The apalis worker this method's deliveries arrive through — the prefix
    /// of every lease its deliveries hold.
    pub(crate) worker: String,
    pub(crate) container: Container,
    /// The queue's storage on the budgeted connection, which hand-backs file
    /// through — never the one apalis fetches with.
    pub(crate) storage: RedisStorage<serde_json::Value, RedisConnection>,
    /// What a record this method's deliveries file back carries: apalis's
    /// attempt cap lifted, and this worker as the one that fetched it — see
    /// `uncapped_context`.
    pub(crate) filing: RedisContext,
    pub(crate) leases: Arc<Leases>,
    /// The shutdown began: no job is fetched any more, and a job settled from
    /// here on may lose its acknowledgement.
    pub(crate) draining: CancellationToken,
    /// The drain window closed: attempts still running are interrupted and
    /// their jobs handed back.
    pub(crate) interrupt: CancellationToken,
}

/// The apalis task a delivery arrived as — what a hand-back rewrites in place.
pub(crate) struct Task {
    pub(crate) id: TaskId,
    pub(crate) attempt: Attempt,
}

impl Task {
    /// The task, carrying `record` in place of what it was fetched with, under
    /// `context` — the worker's filing context, whatever the record was fetched
    /// with: a record filed before apalis's cap was lifted gets the lift at its
    /// first hand-back.
    fn carrying(
        &self,
        record: serde_json::Value,
        context: &RedisContext,
    ) -> Request<serde_json::Value, RedisContext> {
        let mut request = Request::new_with_ctx(record, context.clone());
        request.parts.task_id = self.id.clone();
        request.parts.attempt = self.attempt.clone();
        request
    }
}

/// Why a job goes back to the queue rather than reaching an outcome here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Why {
    /// Its attempt failed and the budget holds another: the next attempt is
    /// filed for when the backoff ends.
    Retry,
    /// Another delivery holds its lease.
    Leased,
    /// Its method's throttle started as many attempts as the window allows.
    Throttled,
    /// The drain window closed on its attempt.
    Shutdown,
    /// Its lease could not be asked for.
    Unguarded,
}

impl Why {
    fn as_str(self) -> &'static str {
        match self {
            Self::Retry => "retry",
            Self::Leased => "leased elsewhere",
            Self::Throttled => "throttled",
            Self::Shutdown => "shutdown",
            Self::Unguarded => "guard unavailable",
        }
    }
}

/// How far a hand-back got.
enum HandBack<E> {
    /// Filed for later and out of flight.
    Whole,
    /// Redis refused the filing, so nothing changed: the task is still in
    /// flight under the record it was fetched with.
    NotScheduled(E),
    /// Filed for later, and Redis refused to take it out of flight.
    StillInFlight(E),
}

impl Deliveries {
    /// apalis's entry point for one fetched job: run its delivery on a task of
    /// its own and answer with what it settled on. The task is aborted if apalis
    /// drops this future — a worker torn down takes its deliveries with it.
    pub(crate) async fn deliver(
        self: Arc<Self>,
        record: serde_json::Value,
        task: Task,
    ) -> Result<(), BoxDynError> {
        // apalis's task id is its record's, never the job's: the job's id is the
        // one the port sealed, and this rides beside it.
        let mut delivery = Delivery::new(&BACKEND, self.queue.clone(), record)
            .with_backend_id(task.id.to_string());
        if self.method.options().checkpoint() {
            let store = RedisCheckpoint::new(self.conn.clone(), &self.queue, delivery.id());
            delivery = delivery.with_checkpoint(Arc::new(store));
        }
        let job = delivery.id().clone();
        let queue = self.queue.clone();
        let running = AbortOnDropHandle::new(tokio::spawn(self.run(delivery, task)));
        match running.await {
            Ok(answer) => answer,
            Err(failure) => failed_outside_the_attempt(&queue, &job, failure),
        }
    }

    /// The delivery itself: admission, the attempt, and how the job settles.
    async fn run(self: Arc<Self>, mut delivery: Delivery, task: Task) -> Result<(), BoxDynError> {
        // The record as it rests: what is handed back when this delivery ends
        // without an outcome. Taken before the attempt, which may consume the
        // stored value on the job's last attempt.
        let resting = delivery.retry_envelope();
        let holder = format!("{}/{}", self.worker, uuid::Uuid::now_v7());
        let admission = self
            .leases
            .admit(delivery.id(), delivery.unique_key(), holder)
            .await;
        let lease = match admission {
            Ok(Admission::Granted(lease)) => lease,
            Ok(Admission::Settled(settlement)) => {
                return self.acknowledge_settled(&delivery, settlement).await;
            }
            Ok(Admission::Cancelled) => {
                // What the job held was let go by the step that met the
                // tombstone, which stays for as long as a settled mark would.
                tracing::info!(
                    target: nest_rs_queue::TARGET,
                    queue = %self.queue,
                    job_id = %delivery.id(),
                    "job delivered after it was cancelled; acknowledged without running",
                );
                return Ok(());
            }
            Ok(Admission::Throttled { ends_in }) => {
                let wait = ends_in.max(SHORTEST_HAND_BACK);
                return self
                    .hand_back(&delivery, &task, resting, wait, Why::Throttled)
                    .await;
            }
            Ok(Admission::Held { holder, lapses_in }) => {
                tracing::info!(
                    target: nest_rs_queue::TARGET,
                    queue = %self.queue,
                    job_id = %delivery.id(),
                    holder = %holder,
                    lapses_in_ms = millis(lapses_in),
                    "job delivered while another delivery runs it; handing it back",
                );
                let wait = lapses_in.max(SHORTEST_HAND_BACK);
                return self
                    .hand_back(&delivery, &task, resting, wait, Why::Leased)
                    .await;
            }
            Err(error) => {
                report_guard(Guard::Unasked, &self.queue, delivery.id(), &error);
                return self
                    .hand_back(
                        &delivery,
                        &task,
                        resting,
                        SHORTEST_HAND_BACK,
                        Why::Unguarded,
                    )
                    .await;
            }
        };

        let renewal = lease.keep();
        let finished = {
            let attempt = consume::attempt(self.method, &mut delivery, self.container.clone());
            tokio::pin!(attempt);
            tokio::select! {
                biased;
                outcome = &mut attempt => Some(outcome),
                () = self.interrupt.cancelled() => None,
            }
        };
        drop(renewal);
        let Some(outcome) = finished else {
            // The drain window closed on the attempt, which is dropped where it
            // stood: the job goes back as it was fetched, for another replica.
            let answer = self
                .hand_back(&delivery, &task, resting, Duration::ZERO, Why::Shutdown)
                .await;
            self.release(&lease, &delivery, Duration::ZERO).await;
            return answer;
        };

        match outcome {
            AttemptOutcome::Ok => {
                self.settle(&lease, &delivery, Settlement::Completed).await;
                Ok(())
            }
            AttemptOutcome::DeadLetter(error) => {
                self.settle(&lease, &delivery, Settlement::DeadLettered)
                    .await;
                Err(dead_letter(error))
            }
            AttemptOutcome::Retry { after } => {
                let next = delivery.retry_envelope();
                let answer = self
                    .hand_back(&delivery, &task, next, after, Why::Retry)
                    .await;
                self.release(&lease, &delivery, after).await;
                answer
            }
        }
    }

    /// A job an earlier delivery already settled: answered as that delivery
    /// answered, without running it. While the worker drains, that answer may
    /// never reach Redis, so the mark is kept as long as a drain's own.
    async fn acknowledge_settled(
        &self,
        delivery: &Delivery,
        settlement: Settlement,
    ) -> Result<(), BoxDynError> {
        tracing::info!(
            target: nest_rs_queue::TARGET,
            queue = %self.queue,
            job_id = %delivery.id(),
            settled = settlement.as_str(),
            "job delivered again after it settled; acknowledged without running",
        );
        if self.draining.is_cancelled()
            && let Err(error) = self
                .leases
                .remember(delivery.id(), SETTLED_WHILE_DRAINING)
                .await
        {
            report_guard(Guard::NotExtended, &self.queue, delivery.id(), &error);
        }
        match settlement {
            Settlement::Completed => Ok(()),
            Settlement::DeadLettered => Err(dead_letter(JobError::abort(
                "dead-lettered by an earlier delivery of this job",
            ))),
        }
    }

    /// Record the job's terminal outcome, and drop its lease. A mark Redis
    /// refused is said and survived: the outcome stands, and only a second
    /// delivery of the job — which the mark exists to stop — could run it again.
    async fn settle(&self, lease: &Lease, delivery: &Delivery, outcome: Settlement) {
        let remember = self
            .draining
            .is_cancelled()
            .then_some(SETTLED_WHILE_DRAINING);
        if let Err(error) = lease.settle(outcome, remember).await {
            report_guard(
                Guard::NotSettled(outcome),
                &self.queue,
                delivery.id(),
                &error,
            );
        }
    }

    /// Drop the lease of a job that goes back to the queue, due again after
    /// `next`. One Redis refused lapses on its own, and until then delays the
    /// job's next delivery.
    async fn release(&self, lease: &Lease, delivery: &Delivery, next: Duration) {
        if let Err(error) = lease.release(next).await {
            report_guard(Guard::NotDropped, &self.queue, delivery.id(), &error);
        }
    }

    /// File `record` as the task's next delivery, due after `wait`, and take the
    /// task out of this worker's flight — so the job goes back to the queue
    /// without depending on an acknowledgement.
    ///
    /// **Onto the schedule first, then out of flight** — two calls, in that
    /// order, so no failure between them loses the job. apalis's `reschedule`
    /// removes the task from the worker's in-flight set *before* it schedules it,
    /// as separate commands, and a failure between the two would leave a job in
    /// neither place; scheduling the same task first means every failure after it
    /// leaves the job scheduled, at worst still in flight as well.
    ///
    /// **Each call is safe to cut at the budget.** A filing that timed out and
    /// ran anyway files the same task under the same id, so the plain failure
    /// that follows files it once more in place, never beside it; a `reschedule`
    /// that timed out and ran leaves the job scheduled and out of flight, which
    /// is where it was going.
    async fn hand_back(
        &self,
        delivery: &Delivery,
        task: &Task,
        record: Envelope,
        wait: Duration,
        why: Why,
    ) -> Result<(), BoxDynError> {
        let at = due_second(SystemTime::now() + wait);
        let record = record.into_json();
        let mut storage = self.storage.clone();
        let outcome = match storage
            .schedule_request(task.carrying(record.clone(), &self.filing), at)
            .await
        {
            Err(error) => HandBack::NotScheduled(error),
            Ok(_) => match storage
                .reschedule(task.carrying(record, &self.filing), wait_until(at))
                .await
            {
                Ok(()) => HandBack::Whole,
                Err(error) => HandBack::StillInFlight(error),
            },
        };
        handed_back(&self.queue, delivery, why, wait, outcome)
    }
}

/// A delivery task that ended outside its attempt — a panic in this crate's own
/// code, since the port catches the handler's — answered as a plain failure,
/// which apalis files again: the job is delivered again rather than lost, and
/// its lease, no longer renewed, lapses before it runs.
fn failed_outside_the_attempt(
    queue: &QueueName,
    job: &JobId,
    failure: tokio::task::JoinError,
) -> Result<(), BoxDynError> {
    let answer = std::io::Error::other(failure.to_string());
    match failure.try_into_panic() {
        Ok(payload) => tracing::error!(
            target: nest_rs_queue::TARGET,
            queue = %queue,
            job_id = %job,
            panic = %panic_message(payload.as_ref()),
            "job delivery failed outside its attempt; it is delivered again",
        ),
        Err(cancelled) => tracing::error!(
            target: nest_rs_queue::TARGET,
            queue = %queue,
            job_id = %job,
            error = %cancelled,
            "job delivery was cancelled outside its attempt; it is delivered again",
        ),
    }
    Err(Box::new(answer))
}

/// A guard call Redis refused.
#[derive(Clone, Copy)]
enum Guard {
    /// Asking for the lease: the job is handed back rather than run unguarded.
    Unasked,
    /// Writing the settled mark — and, in the same step, closing what the job
    /// held: a second delivery would run the job again, a cancel would find it
    /// still open once its lease lapsed, and its unique key stays held until the
    /// week its records are kept runs out.
    NotSettled(Settlement),
    /// Keeping a settled mark past a drain.
    NotExtended,
    /// Dropping the lease: the next delivery waits for it to lapse.
    NotDropped,
}

/// The line a refused guard call files — every one a `warn`, since each leaves
/// the job's delivery guarded less than it should be and says how.
fn report_guard(guard: Guard, queue: &QueueName, job: &JobId, error: &redis::RedisError) {
    let error = nest_rs_core::error_message(error);
    match guard {
        Guard::Unasked => tracing::warn!(
            target: nest_rs_queue::TARGET,
            queue = %queue,
            job_id = %job,
            error = %error,
            "job lease not asked for; the job is handed back rather than run unguarded",
        ),
        Guard::NotSettled(outcome) => tracing::warn!(
            target: nest_rs_queue::TARGET,
            queue = %queue,
            job_id = %job,
            settled = outcome.as_str(),
            error = %error,
            "job settled mark not written; a second delivery would run it again, a cancel could \
             still answer true, and its unique key stays held until it lapses",
        ),
        Guard::NotExtended => tracing::warn!(
            target: nest_rs_queue::TARGET,
            queue = %queue,
            job_id = %job,
            error = %error,
            "job settled mark not extended; if this acknowledgement is lost, the job runs again \
             once the mark lapses",
        ),
        Guard::NotDropped => tracing::warn!(
            target: nest_rs_queue::TARGET,
            queue = %queue,
            job_id = %job,
            error = %error,
            "job lease not dropped; its next delivery waits for it to lapse",
        ),
    }
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

/// What a hand-back answers apalis once Redis has answered, and the line saying
/// which way it went.
///
/// A task Redis would not schedule is failed rather than acknowledged: it is
/// still in flight, and apalis's answer to a plain failure files its stored
/// record back on the schedule, due at once — or, should that answer be lost
/// too, a sweep does. The stored record is the one it was fetched with when
/// Redis refused the filing, and the one filed when a filing that timed out
/// landed all the same: the job runs again at the attempt it was fetched at,
/// or at its next one, sooner than its backoff. The record's context lifts
/// apalis's attempt cap, so no count of earlier deliveries turns the failure
/// into a kill. Acknowledging it would drop the job, and dead-lettering it
/// would bury a job over a failure that was never its own. A task scheduled and
/// still in flight is acknowledged: when the
/// acknowledgement lands it takes the task out of flight, and when it does not,
/// the job is delivered twice — which the line says — and the lease and the
/// settled mark keep the second delivery from running it twice.
fn handed_back<E>(
    queue: &QueueName,
    delivery: &Delivery,
    why: Why,
    wait: Duration,
    outcome: HandBack<E>,
) -> Result<(), BoxDynError>
where
    E: std::error::Error + Send + Sync + 'static,
{
    let job: &JobId = delivery.id();
    match outcome {
        // A retry's own line is the port's (`job failed; will retry within the
        // budget`, at `warn`), so filing it is detail.
        HandBack::Whole if why == Why::Retry => {
            tracing::debug!(
                target: nest_rs_queue::TARGET,
                queue = %queue,
                job_id = %job,
                attempt = delivery.attempt(),
                due_in_ms = millis(wait),
                "job filed for its next attempt",
            );
            Ok(())
        }
        // A throttle holding a job back is the throttle doing what it was
        // declared to do, once per deferred job — detail, like a retry's filing.
        HandBack::Whole if why == Why::Throttled => {
            tracing::debug!(
                target: nest_rs_queue::TARGET,
                queue = %queue,
                job_id = %job,
                attempt = delivery.attempt(),
                due_in_ms = millis(wait),
                "job deferred to its throttle's next window",
            );
            Ok(())
        }
        HandBack::Whole => {
            tracing::info!(
                target: nest_rs_queue::TARGET,
                queue = %queue,
                job_id = %job,
                attempt = delivery.attempt(),
                reason = why.as_str(),
                due_in_ms = millis(wait),
                "job handed back to the queue",
            );
            Ok(())
        }
        HandBack::NotScheduled(error) => {
            tracing::error!(
                target: nest_rs_queue::TARGET,
                queue = %queue,
                job_id = %job,
                reason = why.as_str(),
                error = %nest_rs_core::error_message(&error),
                "job not handed back; apalis files its stored record again, due at once, and it runs \
                 again",
            );
            Err(Box::new(error))
        }
        HandBack::StillInFlight(error) => {
            tracing::warn!(
                target: nest_rs_queue::TARGET,
                queue = %queue,
                job_id = %job,
                attempt = delivery.attempt(),
                reason = why.as_str(),
                error = %nest_rs_core::error_message(&error),
                "job handed back and still in flight; it may be delivered twice",
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

/// `duration` in whole milliseconds, for a line's field.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use nest_rs_testing::LogCapture;

    use super::*;

    fn audio() -> QueueName {
        QueueName::new("audio").expect("a valid name")
    }

    /// A job at its second attempt.
    fn delivery() -> Delivery {
        Delivery::new(
            &BACKEND,
            audio(),
            serde_json::json!({
                "v": nest_rs_queue::WIRE_FORMAT_VERSION,
                "id": "01890a5d-ac96-774b-bcce-b302099a8057",
                "attempt": 2,
                "payload": { "clip": 1 },
            }),
        )
    }

    /// A whole hand-back is said once, naming why and the attempt it hands
    /// back — at `info`, since a job leaving a replica is worth a line.
    #[test]
    fn a_whole_hand_back_is_acknowledged_and_names_why_and_the_attempt() {
        let logs = LogCapture::install();
        let delivery = delivery();
        handed_back::<std::io::Error>(
            &audio(),
            &delivery,
            Why::Shutdown,
            Duration::ZERO,
            HandBack::Whole,
        )
        .expect("acknowledged");

        let event = logs.expect_one(nest_rs_queue::TARGET, "job handed back to the queue");
        assert_eq!(event.level, "info");
        assert_eq!(
            event.field("job_id").as_deref(),
            Some("01890a5d-ac96-774b-bcce-b302099a8057"),
        );
        assert_eq!(event.field("attempt").as_deref(), Some("2"));
        assert_eq!(event.field("reason").as_deref(), Some("shutdown"));
    }

    /// A retry filed whole is detail: the port already said the attempt failed
    /// and will be retried, at `warn`.
    #[test]
    fn a_retry_filed_whole_is_detail_under_the_ports_own_line() {
        let logs = LogCapture::install();
        handed_back::<std::io::Error>(
            &audio(),
            &delivery(),
            Why::Retry,
            Duration::from_secs(2),
            HandBack::Whole,
        )
        .expect("acknowledged");
        let event = logs.expect_one(nest_rs_queue::TARGET, "job filed for its next attempt");
        assert_eq!(event.level, "debug");
        assert_eq!(event.field("due_in_ms").as_deref(), Some("2000"));
        logs.expect_none(nest_rs_queue::TARGET, "job handed back to the queue");
    }

    /// A job its method's throttle held back is detail too: the throttle doing
    /// what it was declared to do, once per deferred job.
    #[test]
    fn a_throttled_job_deferred_whole_is_detail() {
        let logs = LogCapture::install();
        handed_back::<std::io::Error>(
            &audio(),
            &delivery(),
            Why::Throttled,
            Duration::from_secs(3),
            HandBack::Whole,
        )
        .expect("acknowledged");
        let event = logs.expect_one(
            nest_rs_queue::TARGET,
            "job deferred to its throttle's next window",
        );
        assert_eq!(event.level, "debug");
        assert_eq!(event.field("due_in_ms").as_deref(), Some("3000"));
        assert_eq!(
            event.field("attempt").as_deref(),
            Some("2"),
            "not an attempt spent"
        );
        logs.expect_none(nest_rs_queue::TARGET, "job handed back to the queue");
    }

    /// A hand-back Redis refused must neither drop the job nor bury it: the task
    /// fails as a plain error, which apalis re-queues, and the line says the job
    /// is delivered again rather than that it was handed back.
    #[test]
    fn a_hand_back_redis_refuses_fails_the_task_and_says_the_job_runs_again() {
        let logs = LogCapture::install();
        let delivery = delivery();
        let failed = handed_back(
            &audio(),
            &delivery,
            Why::Retry,
            Duration::from_secs(1),
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
            "job not handed back; apalis files its stored record again, due at once, and it runs \
             again",
        );
        assert_eq!(event.level, "error");
        assert_eq!(event.field("job_id"), Some(delivery.id().to_string()));
        assert_eq!(event.field("queue").as_deref(), Some("audio"));
        assert_eq!(event.field("reason").as_deref(), Some("retry"));
        assert_eq!(event.field("error").as_deref(), Some("connection refused"));
        logs.expect_none(nest_rs_queue::TARGET, "job filed for its next attempt");
    }

    /// Scheduled but left in flight: the job may be delivered twice — which is
    /// said at `warn`, since it is a duplicate the guard absorbs and not a loss.
    #[test]
    fn a_hand_back_left_in_flight_is_acknowledged_and_says_it_may_be_delivered_twice() {
        let logs = LogCapture::install();
        let delivery = delivery();
        handed_back(
            &audio(),
            &delivery,
            Why::Leased,
            Duration::from_secs(1),
            HandBack::StillInFlight(std::io::Error::other("connection reset")),
        )
        .expect("acknowledged, so a landing acknowledgement takes it out of flight");

        let event = logs.expect_one(
            nest_rs_queue::TARGET,
            "job handed back and still in flight; it may be delivered twice",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("job_id"), Some(delivery.id().to_string()));
        assert_eq!(event.field("reason").as_deref(), Some("leased elsewhere"));
        assert_eq!(event.field("error").as_deref(), Some("connection reset"));
    }

    /// Every guard call Redis refused is said at `warn`, naming the job and the
    /// cause, in a sentence saying what the refusal costs.
    #[test]
    fn a_guard_call_redis_refuses_is_said_with_what_it_costs() {
        let logs = LogCapture::install();
        let delivery = delivery();
        let refused = redis::RedisError::from(std::io::Error::other("connection reset"));
        for guard in [
            Guard::Unasked,
            Guard::NotSettled(Settlement::Completed),
            Guard::NotExtended,
            Guard::NotDropped,
        ] {
            report_guard(guard, &audio(), delivery.id(), &refused);
        }
        let unasked = logs.expect_one(
            nest_rs_queue::TARGET,
            "job lease not asked for; the job is handed back rather than run unguarded",
        );
        let unsettled = logs.expect_one(
            nest_rs_queue::TARGET,
            "job settled mark not written; a second delivery would run it again, a cancel could \
             still answer true, and its unique key stays held until it lapses",
        );
        let unextended = logs.expect_one(
            nest_rs_queue::TARGET,
            "job settled mark not extended; if this acknowledgement is lost, the job runs again \
             once the mark lapses",
        );
        let undropped = logs.expect_one(
            nest_rs_queue::TARGET,
            "job lease not dropped; its next delivery waits for it to lapse",
        );
        assert_eq!(unsettled.field("settled").as_deref(), Some("completed"));
        for event in [unasked, unsettled, unextended, undropped] {
            assert_eq!(event.level, "warn", "{event:?}");
            assert_eq!(event.field("job_id"), Some(delivery.id().to_string()));
            assert_eq!(event.field("error").as_deref(), Some("connection reset"));
        }
    }

    /// A delivery task that panics outside its attempt answers apalis with a
    /// plain failure — delivered again, never lost, never buried — and says so
    /// at `error` with the panic's message under the one field every containment
    /// seam uses.
    #[tokio::test]
    async fn a_delivery_failing_outside_its_attempt_is_delivered_again_and_said() {
        let logs = LogCapture::install();
        let delivery = delivery();
        let failure = tokio::spawn(async { panic!("lease bookkeeping broke") })
            .await
            .expect_err("the task panicked");
        let answer = failed_outside_the_attempt(&audio(), delivery.id(), failure)
            .expect_err("a plain failure, which apalis files again");
        assert!(
            !answer
                .downcast_ref::<apalis::prelude::Error>()
                .is_some_and(|e| matches!(e, apalis::prelude::Error::Abort(_))),
            "never a dead letter: {answer}",
        );
        let said = logs.expect_one(
            nest_rs_queue::TARGET,
            "job delivery failed outside its attempt; it is delivered again",
        );
        assert_eq!(said.level, "error");
        assert_eq!(
            said.field("panic").as_deref(),
            Some("lease bookkeeping broke")
        );

        let cancelled = tokio::spawn(std::future::pending::<()>());
        cancelled.abort();
        let failure = cancelled.await.expect_err("the task was cancelled");
        failed_outside_the_attempt(&audio(), delivery.id(), failure)
            .expect_err("a plain failure, which apalis files again");
        let said = logs.expect_one(
            nest_rs_queue::TARGET,
            "job delivery was cancelled outside its attempt; it is delivered again",
        );
        assert_eq!(said.level, "error");
    }

    /// A hand-back files the task under the worker's filing context — apalis's
    /// attempt cap lifted, this worker as the one that fetched it — whatever the
    /// record was fetched with, and keeps the task's id and the deliveries apalis
    /// counted.
    #[test]
    fn a_hand_back_files_the_task_under_the_workers_uncapped_context() {
        let filing = crate::backend::uncapped_context(Some("host:01")).expect("apalis's form");
        let task = Task {
            id: TaskId::new(),
            attempt: Attempt::new_with_value(6),
        };
        let filed = task.carrying(serde_json::json!({ "clip": 1 }), &filing);
        assert_eq!(filed.parts.task_id, task.id);
        assert_eq!(filed.parts.attempt.current(), 6);
        let context = serde_json::to_value(&filed.parts.context).expect("serializes");
        assert_eq!(
            context["max_attempts"],
            serde_json::Value::from(u32::MAX),
            "{context}"
        );
        assert_eq!(
            context,
            serde_json::to_value(&filing).expect("serializes"),
            "the filing context, not what the record was fetched with",
        );
    }

    /// A hand-back waits at least as long as asked, counted in apalis's whole
    /// seconds.
    #[test]
    fn a_hand_back_lands_on_the_second_the_wait_ends_or_after_it() {
        let now = due_second(SystemTime::now());
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
