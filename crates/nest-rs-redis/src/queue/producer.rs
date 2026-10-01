//! [`RedisQueueProducer`] — the queue port's producer half over the shared
//! [`RedisConnection`]: the [`JobProducer`] binding a feature injects as
//! `Arc<dyn JobProducer>` to push without naming the backend.
//!
//! The port checks the queue and the options, mints each job's id and seals
//! every job before this type sees it; what is stored is that sealed envelope,
//! as the port handed it, on an apalis `RedisStorage<serde_json::Value>` under
//! the queue's namespace (`nestrs:queue:<queue>`, see [`crate::layout`]) —
//! which is how apalis routes a job to the worker draining that queue. Every job
//! is a JSON `Value` at this boundary, so the worker never names the payload
//! type. The receipt a caller gets names the port's id; apalis's own task id for
//! the record stays inside this crate, and reaches a job's lines only as its
//! `backend_id`.
//!
//! A push holding the job back files it on the queue's schedule instead of its
//! list, due on the second its delay ends, and the producer's [`Promoter`]
//! moves it onto the list once due. Either way the record carries a context
//! whose apalis attempt cap no count of deliveries reaches, so apalis's count
//! never ends the job (see `uncapped_context`).
//!
//! **Before a job is filed, its records are opened** — its `open` mark, and the
//! claim on the unique key it was pushed under (see [`crate::layout`]) — so a
//! cancel can tell a job still waiting from one long finished, and a second push
//! under a held key is refused before anything is filed. The claim is atomic: one
//! script reads the key and claims it, so two pushes racing for one key file one
//! job. A filing Redis refuses closes what the push opened, since nothing was
//! filed; one whose answer never came leaves them open, since the job may be on
//! the queue — and they lapse [`KEPT_PAST_DUE`] after the job was due if it is
//! not.
//!
//! **A claim is held briefly until its job is known to be queued.** It is taken
//! for [`CLAIM_HOLD`] and extended to the job's full keeping once Redis confirms
//! the filing. A push that never learns whether its job was queued — its claim or
//! its filing unanswered, the call dropped by a caller or by the port's net —
//! leaves a claim that lapses within the hold, and says so at `warn`: a key held
//! for a week by a job that was never filed silently refused every retry under
//! it. The direction this errs in is at-least-once: a job that *was* queued and
//! whose claim lapses before a worker admits it may be pushed again under its key.
//!
//! **A cancel is a tombstone.** apalis's structures are never touched: a cancel
//! writes `cancelled` beside the job — atomically against the delivery guard's
//! lease, so it answers `true` only while no attempt runs — and closes what the
//! job held; the delivery that meets the tombstone acknowledges the job without
//! running it.
//!
//! **A call fails within one connection budget of Redis going silent.** Each
//! command is bounded by the connection, and no call sends a second one after a
//! command that got no answer: the 6.x check runs beside the filing rather than
//! in front of it, and a filing that timed out leaves what it opened to lapse.
//! So a silent Redis reaches the caller as the connection's own failure, with
//! its cause, before the port's net (`nest_rs_queue::BACKEND_TIMEOUT`) gives up
//! on the call — and when the net is what ends one, the call is dropped where it
//! stands, which leaves nothing its failure would not: marks that lapse, and
//! records already filed, already watched by the promoter.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use apalis::prelude::{Request, Storage};
use apalis_redis::RedisStorage;
use async_trait::async_trait;
use nest_rs_queue::{
    BACKEND_TIMEOUT, Envelope, JobId, JobProducer, PushOptions, QueueBackend, QueueError, QueueName,
};
use redis::Script;

use super::promoter::Promoter;
use crate::RedisConnection;
use crate::backend::{BACKEND, due_second, uncapped_context};
use crate::layout::{self, CANCELLED, CHECKPOINTS, KEPT_PAST_DUE, LEASES, OPEN, job_key, millis};
use crate::legacy_layout::{self, LegacyLayout};

/// How long a unique key is claimed before its job is confirmed queued: twice
/// the port's net. Every call the port makes is dropped at the net
/// ([`BACKEND_TIMEOUT`]), so a push still going confirms its filing — and
/// extends the claim — well inside the hold, and one that never will lets the
/// key go within it.
pub(crate) const CLAIM_HOLD: Duration = Duration::from_secs(BACKEND_TIMEOUT.as_secs() * 2);

/// Open a job pushed under a unique key: claim the key for it and open its
/// record, unless another job holds the key — whose id it answers, or `''` when
/// the key was free and is now this job's.
///
/// `KEYS`: the claim, the job's open record. `ARGV`: the job's id, how long the
/// open record is kept, the unique key, how long the claim is held until the
/// job is confirmed queued.
const CLAIM: &str = r"
local holder = redis.call('GET', KEYS[1])
if holder then
  return holder
end
redis.call('SET', KEYS[1], ARGV[1], 'PX', ARGV[4])
redis.call('SET', KEYS[2], ARGV[3], 'PX', ARGV[2])
return ''
";

/// Keep a claim for its job's full keeping, if the job still holds it: `1`
/// extended, `0` no longer the job's — settled or cancelled meanwhile.
///
/// `KEYS`: the claim. `ARGV`: the job's id, how long it is kept.
const KEEP: &str = r"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('PEXPIRE', KEYS[1], ARGV[2])
end
return 0
";

/// Close what a push opened for a job it did not file: its open record, and
/// its claim when the job still holds it.
///
/// `KEYS`: the open record, then the claim when the job was pushed under a
/// unique key. `ARGV`: the job's id.
const CLOSE: &str = r"
redis.call('DEL', KEYS[1])
if KEYS[2] and redis.call('GET', KEYS[2]) == ARGV[1] then
  redis.call('DEL', KEYS[2])
end
return 1
";

/// Cancel a job that has not started: `1` when its open record still names the
/// unique key the caller read and no delivery holds its lease — the tombstone is
/// then written, for as long as the job's records would have been kept, and what
/// the job held is closed — `0` otherwise, touching nothing.
///
/// `KEYS`: open, lease, cancelled, checkpoints, then the claim when the job
/// holds one. `ARGV`: the unique key the open record holds (`''` for none), the
/// job's id, the shortest the tombstone is kept.
const CANCEL: &str = r"
if redis.call('GET', KEYS[1]) ~= ARGV[1] then
  return 0
end
if redis.call('EXISTS', KEYS[2]) == 1 then
  return 0
end
local kept = redis.call('PTTL', KEYS[1])
if kept < tonumber(ARGV[3]) then
  kept = tonumber(ARGV[3])
end
redis.call('SET', KEYS[3], ARGV[2], 'PX', kept)
redis.call('DEL', KEYS[1], KEYS[4])
if KEYS[5] and redis.call('GET', KEYS[5]) == ARGV[2] then
  redis.call('DEL', KEYS[5])
end
return 1
";

/// The producer a feature pushes through. Bound by
/// [`RedisQueueModule`](crate::RedisQueueModule) under both its own name and
/// `Arc<dyn JobProducer>`; a `Clone` shares the underlying connection, the
/// delayed records it watches and the queues it has checked.
///
/// It honours every capability the crate's backend declaration names: a delay,
/// a unique key, a cancel by receipt or by unique key.
#[derive(Clone)]
pub struct RedisQueueProducer {
    conn: RedisConnection,
    promoter: Promoter,
    /// The queues whose 6.x keys this producer has looked for — once per queue
    /// per process, so the check costs one round trip the first time and never
    /// again.
    checked: Arc<Mutex<HashSet<QueueName>>>,
    claim: Script,
    keep: Script,
    close: Script,
    cancel: Script,
}

impl RedisQueueProducer {
    /// A producer over the app's shared connection (reused, never reopened).
    pub fn new(conn: RedisConnection) -> Self {
        Self {
            conn,
            promoter: Promoter::default(),
            checked: Arc::default(),
            claim: Script::new(CLAIM),
            keep: Script::new(KEEP),
            close: Script::new(CLOSE),
            cancel: Script::new(CANCEL),
        }
    }

    /// Producer-side storage handle under `queue`'s namespace, the one the
    /// worker draining it reads.
    fn storage(&self, queue: &QueueName) -> RedisStorage<serde_json::Value, RedisConnection> {
        RedisStorage::new_with_config(self.conn.clone(), layout::config(queue))
    }

    /// Say once per queue when jobs still wait under the 6.x layout: this push
    /// lands where a 7.0 worker reads, and those do not. The worker refuses to
    /// start beside them; a producer has nothing to refuse — its push is right —
    /// so it warns, and keeps pushing.
    async fn look_for_legacy_jobs(&self, queue: &QueueName) {
        let first = self
            .checked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(queue.clone());
        if !first {
            return;
        }
        let found = LegacyLayout::read(&self.conn, queue).await;
        report_legacy_check(queue, found.map(|found| found.held()));
    }

    /// File `envelopes` on `queue` as `options` say: open their records, then
    /// file each on the queue's list, or on its schedule when held back.
    async fn file(
        &self,
        queue: &QueueName,
        envelopes: Vec<Envelope>,
        options: &PushOptions,
    ) -> Result<(), QueueError> {
        // apalis's own attempt cap, lifted on every record filed here — see
        // `uncapped_context`. Built before anything is opened, so a refusal
        // leaves nothing behind.
        let context = uncapped_context(None).map_err(QueueError::backend)?;
        let now = SystemTime::now();
        let due = match options.delay() {
            None => None,
            // An instant already past is an immediate push.
            Some(delay) => Some(delay.deadline(now)?).filter(|at| *at > now),
        };
        let until_due = due
            .and_then(|at| at.duration_since(now).ok())
            .unwrap_or_default();
        let kept = until_due.saturating_add(KEPT_PAST_DUE);
        // Every claim this push takes is watched until its job is confirmed
        // queued, or its fate said: a push dropped in between says so as it goes.
        let mut unconfirmed = Unconfirmed::new(queue);
        let opened = self.open(queue, &envelopes, kept, &mut unconfirmed).await?;

        // `push` takes `&mut self`; storage is a cheap clone of the connection
        // handle, so build one per call rather than force callers to hold it mut.
        let mut storage = self.storage(queue);
        let second = due.map(due_second);
        for (at, envelope) in envelopes.into_iter().enumerate() {
            let record = Request::new_with_ctx(envelope.into_json(), context.clone());
            let filed = match second {
                None => storage.push_request(record).await.map(drop),
                Some(second) => storage.schedule_request(record, second).await.map(drop),
            };
            let error = match filed {
                Ok(()) => {
                    // Watched from the first record filed, so a push cut short
                    // after it — by a failure below, or by the port's net
                    // dropping the call — still has what it filed promoted.
                    if at == 0
                        && let Some(second) = second
                    {
                        self.promoter.watch(&self.conn, queue, second);
                    }
                    if let Some(key) = &opened[at].unique {
                        self.keep_claim(queue, &opened[at].id, key, kept).await;
                        unconfirmed.said(&opened[at].id);
                    }
                    continue;
                }
                Err(error) => error,
            };
            // What was never filed is closed; the job whose filing failed is
            // closed only when Redis answered, since otherwise it may be queued.
            // A filing Redis did not answer within the budget closes nothing
            // more: a close would spend a second budget on a Redis that just
            // went silent, and the marks lapse on their own — none holds a
            // unique key, which only a push of one job carries.
            let failed = &opened[at];
            let unfiled = if answered(&error) {
                &opened[at..]
            } else {
                if let Some(key) = &failed.unique {
                    report_short_hold(queue, &failed.id, key, Step::FilingUnanswered, Some(&error));
                }
                if error.is_timeout() {
                    &[]
                } else {
                    &opened[at + 1..]
                }
            };
            self.close_quietly(queue, unfiled).await;
            unconfirmed.all_said();
            return Err(QueueError::backend(error));
        }
        Ok(())
    }

    /// Open the records of every job in `envelopes` before any is filed, each
    /// kept for `kept`: the claim on its unique key and its open mark. A key
    /// another job holds refuses the push naming that job — closing what this
    /// push had already opened — and nothing is filed. Answers what it opened,
    /// job by job, in the order of `envelopes`.
    async fn open(
        &self,
        queue: &QueueName,
        envelopes: &[Envelope],
        kept: Duration,
        unconfirmed: &mut Unconfirmed,
    ) -> Result<Vec<Opened>, QueueError> {
        let kept = millis(kept);
        let mut plain = redis::pipe();
        let mut opened: Vec<Opened> = Vec::with_capacity(envelopes.len());
        for envelope in envelopes {
            let job = Opened {
                id: envelope.id().clone(),
                unique: envelope.unique_key().map(str::to_owned),
            };
            let Some(key) = &job.unique else {
                plain
                    .cmd("SET")
                    .arg(job_key(OPEN, queue, &job.id))
                    .arg("")
                    .arg("PX")
                    .arg(kept)
                    .ignore();
                opened.push(job);
                continue;
            };
            // Watched from before the claim is sent: a claim dropped in flight
            // may have landed.
            unconfirmed.watch(&job.id, key);
            let claimed = self
                .claim
                .key(layout::unique_key(queue, key))
                .key(job_key(OPEN, queue, &job.id))
                .arg(job.id.to_string())
                .arg(kept)
                .arg(key)
                .arg(millis(CLAIM_HOLD))
                .invoke_async::<String>(&mut self.conn.clone())
                .await;
            let holder = match claimed {
                Ok(holder) => holder,
                Err(error) => {
                    // Refused, the claim was not taken; unanswered, it may have
                    // been, and nothing will confirm it.
                    if !answered(&error) {
                        report_short_hold(queue, &job.id, key, Step::ClaimUnanswered, Some(&error));
                    }
                    self.close_quietly(queue, &opened).await;
                    unconfirmed.all_said();
                    return Err(QueueError::backend(error));
                }
            };
            if !holder.is_empty() {
                self.close_quietly(queue, &opened).await;
                unconfirmed.all_said();
                return Err(QueueError::UniqueKeyHeld {
                    queue: queue.clone(),
                    key: key.clone(),
                    holder: JobId::parse(&holder)?,
                });
            }
            opened.push(job);
        }
        if opened.iter().any(|job| job.unique.is_none()) {
            plain
                .query_async::<()>(&mut self.conn.clone())
                .await
                .map_err(QueueError::backend)?;
        }
        Ok(opened)
    }

    /// Keep `job`'s claim on `key` for `kept`, now that its filing is confirmed.
    /// A claim Redis did not extend lapses within [`CLAIM_HOLD`], which is said;
    /// one no longer the job's — a worker settled it meanwhile — is left alone.
    async fn keep_claim(&self, queue: &QueueName, job: &JobId, key: &str, kept: Duration) {
        let kept = self
            .keep
            .key(layout::unique_key(queue, key))
            .arg(job.to_string())
            .arg(millis(kept))
            .invoke_async::<i64>(&mut self.conn.clone())
            .await;
        if let Err(error) = kept {
            report_short_hold(queue, job, key, Step::NotExtended, Some(&error));
        }
    }

    /// Close the records of `jobs`, which this push opened and did not file —
    /// the open marks of all of them in one command, and each unique claim a job
    /// still holds. A close Redis refuses leaves them to lapse on their own; a
    /// unique key left held that way is said, since a push under it is refused
    /// until then.
    async fn close_quietly(&self, queue: &QueueName, jobs: &[Opened]) {
        let plain: Vec<String> = jobs
            .iter()
            .filter(|job| job.unique.is_none())
            .map(|job| job_key(OPEN, queue, &job.id))
            .collect();
        if !plain.is_empty()
            && let Err(error) = redis::cmd("DEL")
                .arg(&plain)
                .query_async::<i64>(&mut self.conn.clone())
                .await
        {
            // Detail: nothing holds a key here, so nothing is left blocked — the
            // marks lapse on their own, and a cancel of a job never filed says
            // `true` truthfully meanwhile.
            tracing::debug!(
                target: nest_rs_queue::TARGET,
                queue = %queue,
                jobs = plain.len(),
                error = %nest_rs_core::error_message(&error),
                "open marks of jobs a failed push never filed not closed; they lapse on their own",
            );
        }
        for job in jobs {
            let Some(key) = &job.unique else {
                continue;
            };
            let closed = self
                .close
                .key(job_key(OPEN, queue, &job.id))
                .key(layout::unique_key(queue, key))
                .arg(job.id.to_string())
                .invoke_async::<i64>(&mut self.conn.clone())
                .await;
            if let Err(error) = closed {
                report_left_held(queue, &job.id, key, &error);
            }
        }
    }

    /// Cancel `job` on `queue`, whose open record holds `unique` (empty for
    /// none): whether the job had not started and now never will.
    async fn cancel_open(
        &self,
        queue: &QueueName,
        job: &JobId,
        unique: &str,
    ) -> Result<bool, QueueError> {
        let mut invocation = self.cancel.key(job_key(OPEN, queue, job));
        invocation
            .key(job_key(LEASES, queue, job))
            .key(job_key(CANCELLED, queue, job))
            .key(job_key(CHECKPOINTS, queue, job));
        if !unique.is_empty() {
            invocation.key(layout::unique_key(queue, unique));
        }
        let cancelled: i64 = invocation
            .arg(unique)
            .arg(job.to_string())
            .arg(millis(KEPT_PAST_DUE))
            .invoke_async(&mut self.conn.clone())
            .await
            .map_err(QueueError::backend)?;
        Ok(cancelled == 1)
    }
}

/// What a push opened for one job before filing it: the job's id, and the
/// unique key it claimed, if any — what closing it again needs.
struct Opened {
    id: JobId,
    unique: Option<String>,
}

/// Whether Redis answered a failed command — refused it, rather than never
/// replying. Only an answer says the command did not run: a timeout, a dropped
/// connection or a refused dial leaves it unknown, since the write may have
/// landed and its reply not.
fn answered(error: &redis::RedisError) -> bool {
    !error.is_io_error()
}

/// What the producer's one look at the 6.x layout says: jobs waiting there at
/// `warn`, since no 7.0 worker will run them; a check the ACL refused as detail,
/// since a user confined to the framework's keys could not have written them;
/// any other refusal at `warn`, since those jobs would then go unnoticed.
fn report_legacy_check(queue: &QueueName, outcome: Result<Vec<String>, redis::RedisError>) {
    match outcome {
        Ok(keys) if keys.is_empty() => {}
        Ok(keys) => tracing::warn!(
            target: nest_rs_queue::TARGET,
            queue = %queue,
            keys = %keys.join(", "),
            namespace = %layout::namespace(queue),
            "jobs wait under the 6.x key layout; a 7.0 worker does not run them — drain them \
             with a 6.x worker, or move them under the queue's namespace",
        ),
        Err(error) if legacy_layout::outside_the_acl(&error) => tracing::debug!(
            target: nest_rs_queue::TARGET,
            queue = %queue,
            error = %nest_rs_core::error_message(&error),
            "6.x key layout not checked: the connection's ACL does not reach it",
        ),
        Err(error) => tracing::warn!(
            target: nest_rs_queue::TARGET,
            queue = %queue,
            error = %nest_rs_core::error_message(&error),
            "6.x key layout not checked; jobs waiting there would go unnoticed",
        ),
    }
}

/// A unique key a push claimed and could not let go of: the push failed, and
/// a push under the key is refused, naming `job`, until the claim lapses or a
/// `cancel_unique` frees it.
fn report_left_held(queue: &QueueName, job: &JobId, key: &str, error: &redis::RedisError) {
    tracing::warn!(
        target: nest_rs_queue::TARGET,
        queue = %queue,
        job_id = %job,
        unique_key = key,
        kept_ms = millis(KEPT_PAST_DUE),
        error = %nest_rs_core::error_message(error),
        "unique key left held by a push that did not file its job; cancel_unique frees it, and \
         it lapses on its own otherwise",
    );
}

/// Why a push could not confirm the job its unique claim names was queued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// The claim got no answer: it may have been taken, and the job was not
    /// filed.
    ClaimUnanswered,
    /// The filing got no answer: the job may be queued.
    FilingUnanswered,
    /// The job is queued, and Redis did not extend its claim.
    NotExtended,
    /// The push was dropped — by its caller, or by the port's net — between
    /// the claim and the confirmation: the job may be queued.
    Dropped,
}

impl Step {
    fn as_str(self) -> &'static str {
        match self {
            Self::ClaimUnanswered => "claim unanswered",
            Self::FilingUnanswered => "filing unanswered",
            Self::NotExtended => "claim not extended",
            Self::Dropped => "push dropped",
        }
    }
}

/// A unique claim whose job the push could not confirm queued: the key is held
/// for [`CLAIM_HOLD`] at most — longer only if a worker admits the job, which
/// renews it — and a push under it is refused, naming `job`, until then.
fn report_short_hold(
    queue: &QueueName,
    job: &JobId,
    key: &str,
    step: Step,
    error: Option<&redis::RedisError>,
) {
    tracing::warn!(
        target: nest_rs_queue::TARGET,
        queue = %queue,
        job_id = %job,
        unique_key = key,
        step = step.as_str(),
        held_ms = millis(CLAIM_HOLD),
        error = error.map(|error| tracing::field::display(nest_rs_core::error_message(error))),
        "unique key claimed without its job confirmed queued; the claim lapses within its hold \
         unless a worker admits the job",
    );
}

/// The unique claims a push took and has not yet confirmed or said: dropped
/// with any left — the push cut short between a claim and its confirmation —
/// each is said as it goes.
struct Unconfirmed {
    queue: QueueName,
    claims: Vec<(JobId, String)>,
}

impl Unconfirmed {
    fn new(queue: &QueueName) -> Self {
        Self {
            queue: queue.clone(),
            claims: Vec::new(),
        }
    }

    fn watch(&mut self, job: &JobId, key: &str) {
        self.claims.push((job.clone(), key.to_owned()));
    }

    /// `job`'s claim is confirmed, or its fate already said.
    fn said(&mut self, job: &JobId) {
        self.claims.retain(|(held, _)| held != job);
    }

    /// Every claim's fate is said, or closed.
    fn all_said(&mut self) {
        self.claims.clear();
    }
}

impl Drop for Unconfirmed {
    fn drop(&mut self) {
        for (job, key) in &self.claims {
            report_short_hold(&self.queue, job, key, Step::Dropped, None);
        }
    }
}

#[async_trait]
impl JobProducer for RedisQueueProducer {
    fn backend(&self) -> &'static QueueBackend {
        &BACKEND
    }

    async fn enqueue(
        &self,
        queue: &QueueName,
        envelopes: Vec<Envelope>,
        options: &PushOptions,
    ) -> Result<(), QueueError> {
        // Beside the filing, not in front of it: the check only ever warns, and
        // ahead of the filing it would spend a second budget on a Redis that
        // stopped answering — the first push to each queue then waiting two.
        let (_, filed) = tokio::join!(
            self.look_for_legacy_jobs(queue),
            self.file(queue, envelopes, options),
        );
        filed
    }

    async fn remove(&self, queue: &QueueName, id: &JobId) -> Result<bool, QueueError> {
        let open: Option<String> = redis::cmd("GET")
            .arg(job_key(OPEN, queue, id))
            .query_async(&mut self.conn.clone())
            .await
            .map_err(QueueError::backend)?;
        match open {
            Some(unique) => self.cancel_open(queue, id, &unique).await,
            None => Ok(false),
        }
    }

    async fn remove_unique(&self, queue: &QueueName, key: &str) -> Result<bool, QueueError> {
        let holder: Option<String> = redis::cmd("GET")
            .arg(layout::unique_key(queue, key))
            .query_async(&mut self.conn.clone())
            .await
            .map_err(QueueError::backend)?;
        match holder {
            Some(holder) => self.cancel_open(queue, &JobId::parse(&holder)?, key).await,
            None => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_testing::LogCapture;

    use super::*;

    /// A check Redis refused for any reason but the ACL is a `warn`: the jobs it
    /// was looking for would otherwise wait with nothing to say so. One the ACL
    /// refused is detail, and a check that found nothing says nothing.
    #[test]
    fn a_6x_check_redis_refuses_is_said_and_one_the_acl_refuses_is_detail() {
        let logs = LogCapture::install();
        let audio = QueueName::new("audio").expect("a valid name");
        report_legacy_check(&audio, Ok(Vec::new()));
        report_legacy_check(
            &audio,
            Err(redis::RedisError::from(std::io::Error::other("timed out"))),
        );
        report_legacy_check(
            &audio,
            Err(redis::make_extension_error(
                "NOPERM".to_owned(),
                Some("this user has no permissions to access one of the keys".to_owned()),
            )),
        );
        let unchecked = logs.expect_one(
            nest_rs_queue::TARGET,
            "6.x key layout not checked; jobs waiting there would go unnoticed",
        );
        assert_eq!(unchecked.level, "warn");
        assert_eq!(unchecked.field("error").as_deref(), Some("timed out"));
        assert_eq!(logs.events().len(), 2, "the empty check said nothing");
    }

    /// Only an answer says a filing did not run: a refusal Redis sent back
    /// closes what the push opened, while a timeout, a reset or a refused dial
    /// leaves it open, since the job may be queued.
    #[test]
    fn a_filing_redis_refused_is_known_not_to_have_run_and_one_unanswered_is_not() {
        let refusals = [
            redis::RedisError::from((redis::ErrorKind::ResponseError, "ERR", "wrong type".into())),
            redis::make_extension_error("NOPERM".to_owned(), Some("no access".to_owned())),
            redis::RedisError::from((redis::ErrorKind::ReadOnly, "READONLY", "replica".into())),
        ];
        for refusal in &refusals {
            assert!(answered(refusal), "{refusal}");
        }
        for kind in [
            std::io::ErrorKind::TimedOut,
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::ConnectionRefused,
            std::io::ErrorKind::UnexpectedEof,
        ] {
            let unknown = redis::RedisError::from(std::io::Error::from(kind));
            assert!(!answered(&unknown), "{unknown}");
        }
    }

    /// A unique key a failed push leaves held is said at `warn` with the job
    /// that holds it and the key — what a later refusal names, and what an
    /// operator greps for.
    #[test]
    fn a_unique_key_a_failed_push_leaves_held_is_said_with_the_job_and_the_key() {
        let logs = LogCapture::install();
        let audio = QueueName::new("audio").expect("a valid name");
        let job = JobId::parse("01890a5d-ac96-774b-bcce-b302099a8057").expect("a job id");
        let reset = redis::RedisError::from(std::io::Error::other("connection reset"));
        report_short_hold(&audio, &job, "clip-1", Step::FilingUnanswered, Some(&reset));
        report_left_held(&audio, &job, "clip-1", &reset);

        let unconfirmed = logs.expect_one(nest_rs_queue::TARGET, SHORT_HOLD);
        let left = logs.expect_one(
            nest_rs_queue::TARGET,
            "unique key left held by a push that did not file its job; cancel_unique frees it, \
             and it lapses on its own otherwise",
        );
        assert_eq!(
            unconfirmed.field("step").as_deref(),
            Some("filing unanswered")
        );
        assert_eq!(
            unconfirmed.field("held_ms"),
            Some(millis(CLAIM_HOLD).to_string())
        );
        for event in [unconfirmed, left] {
            assert_eq!(event.level, "warn", "{event:?}");
            assert_eq!(event.field("job_id"), Some(job.to_string()));
            assert_eq!(event.field("unique_key").as_deref(), Some("clip-1"));
            assert_eq!(event.field("error").as_deref(), Some("connection reset"));
        }
    }

    const SHORT_HOLD: &str = "unique key claimed without its job confirmed queued; the claim \
                              lapses within its hold unless a worker admits the job";

    /// A push dropped between its claim and the confirmation — by its caller,
    /// or by the port's net — says so as it goes, naming the job and the key: it
    /// said nothing, and the claim it left refused every retry for a week.
    #[test]
    fn a_push_dropped_before_its_claim_is_confirmed_says_so_as_it_goes() {
        let logs = LogCapture::install();
        let audio = QueueName::new("audio").expect("a valid name");
        let job = |id: &str| JobId::parse(id).expect("a job id");
        let dropped = job("01890a5d-ac96-774b-bcce-b302099a8057");
        let confirmed = job("01890a5d-ac96-774b-bcce-b302099a8058");
        {
            let mut unconfirmed = Unconfirmed::new(&audio);
            unconfirmed.watch(&dropped, "clip-1");
            unconfirmed.watch(&confirmed, "clip-2");
            unconfirmed.said(&confirmed);
        }
        let said = logs.expect_one(nest_rs_queue::TARGET, SHORT_HOLD);
        assert_eq!(said.level, "warn");
        assert_eq!(said.field("job_id"), Some(dropped.to_string()));
        assert_eq!(said.field("unique_key").as_deref(), Some("clip-1"));
        assert_eq!(said.field("step").as_deref(), Some("push dropped"));
        assert!(said.field("error").is_none(), "nothing failed: {said:?}");

        let mut settled = Unconfirmed::new(&audio);
        settled.watch(&job("01890a5d-ac96-774b-bcce-b302099a8059"), "clip-3");
        settled.all_said();
        drop(settled);
        assert_eq!(logs.find(nest_rs_queue::TARGET, SHORT_HOLD).len(), 1);
    }

    /// The hold outlasts every call the port waits on, so a push still going
    /// always confirms inside it.
    #[test]
    fn a_claim_is_held_past_the_ports_net() {
        assert!(CLAIM_HOLD > BACKEND_TIMEOUT);
        assert!(CLAIM_HOLD < KEPT_PAST_DUE);
    }
}
