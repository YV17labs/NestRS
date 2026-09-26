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
//! whose apalis attempt cap no count of deliveries reaches, so apalis never ends
//! a job the port has not (see `uncapped_context`).
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
//! **A cancel is a tombstone.** apalis's structures are never touched: a cancel
//! writes `cancelled` beside the job — atomically against the delivery guard's
//! lease, so it answers `true` only while no attempt runs — and closes what the
//! job held; the delivery that meets the tombstone acknowledges the job without
//! running it.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use apalis::prelude::{Request, Storage};
use apalis_redis::RedisStorage;
use async_trait::async_trait;
use nest_rs_queue::{
    Envelope, JobId, JobProducer, PushOptions, QueueBackend, QueueError, QueueName,
};
use redis::Script;

use super::promoter::Promoter;
use crate::RedisConnection;
use crate::backend::{BACKEND, due_second, uncapped_context};
use crate::layout::{self, CANCELLED, CHECKPOINTS, KEPT_PAST_DUE, LEASES, OPEN, job_key, millis};

/// Open a job pushed under a unique key: claim the key for it and open its
/// record, unless another job holds the key — whose id it answers, or `''` when
/// the key was free and is now this job's.
///
/// `KEYS`: the claim, the job's open record. `ARGV`: the job's id, how long both
/// are kept, the unique key.
const CLAIM: &str = r"
local holder = redis.call('GET', KEYS[1])
if holder then
  return holder
end
redis.call('SET', KEYS[1], ARGV[1], 'PX', ARGV[2])
redis.call('SET', KEYS[2], ARGV[3], 'PX', ARGV[2])
return ''
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
        report_legacy_check(queue, layout::legacy_jobs(&self.conn, queue).await);
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
            let holder: String = self
                .claim
                .key(layout::unique_key(queue, key))
                .key(job_key(OPEN, queue, &job.id))
                .arg(job.id.to_string())
                .arg(kept)
                .arg(key)
                .invoke_async(&mut self.conn.clone())
                .await
                .map_err(QueueError::backend)?;
            if !holder.is_empty() {
                self.close_quietly(queue, &opened).await;
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
        Err(error) if layout::outside_the_acl(&error) => tracing::debug!(
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

/// A push whose filing went unanswered, pushed under a unique key: the job may
/// be on the queue, so the key stays held by it — which is what a push under the
/// key is refused naming.
fn report_unanswered(queue: &QueueName, job: &JobId, key: &str, error: &redis::RedisError) {
    tracing::warn!(
        target: nest_rs_queue::TARGET,
        queue = %queue,
        job_id = %job,
        unique_key = key,
        error = %nest_rs_core::error_message(error),
        "job push unanswered; the job may be queued, so its unique key stays held by it",
    );
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
        self.look_for_legacy_jobs(queue).await;
        // apalis's own attempt cap, lifted on every record filed here — see
        // `uncapped_context`. Built before anything is opened, so a refusal
        // leaves nothing behind.
        let context = uncapped_context(None).map_err(QueueError::backend)?;
        let now = SystemTime::now();
        let due = match options.delay() {
            None => None,
            Some(delay) => match delay.deadline(now) {
                // An instant already past is an immediate push.
                Some(at) if at <= now => None,
                Some(at) => Some(at),
                None => {
                    return Err(QueueError::InvalidOptions {
                        reason: "the delay ends past the last instant the clock can represent",
                    });
                }
            },
        };
        let until_due = due
            .and_then(|at| at.duration_since(now).ok())
            .unwrap_or_default();
        let opened = self
            .open(queue, &envelopes, until_due.saturating_add(KEPT_PAST_DUE))
            .await?;

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
            let Err(error) = filed else {
                continue;
            };
            // What was never filed is closed; the job whose filing failed is
            // closed only when Redis answered, since otherwise it may be queued.
            let failed = &opened[at];
            let unfiled = if answered(&error) {
                &opened[at..]
            } else {
                if let Some(key) = &failed.unique {
                    report_unanswered(queue, &failed.id, key, &error);
                }
                &opened[at + 1..]
            };
            self.close_quietly(queue, unfiled).await;
            if at > 0
                && let Some(second) = second
            {
                self.promoter.watch(&self.conn, queue, second);
            }
            return Err(QueueError::backend(error));
        }
        if let Some(second) = second {
            self.promoter.watch(&self.conn, queue, second);
        }
        Ok(())
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
        report_unanswered(&audio, &job, "clip-1", &reset);
        report_left_held(&audio, &job, "clip-1", &reset);

        let unanswered = logs.expect_one(
            nest_rs_queue::TARGET,
            "job push unanswered; the job may be queued, so its unique key stays held by it",
        );
        let left = logs.expect_one(
            nest_rs_queue::TARGET,
            "unique key left held by a push that did not file its job; cancel_unique frees it, \
             and it lapses on its own otherwise",
        );
        for event in [unanswered, left] {
            assert_eq!(event.level, "warn", "{event:?}");
            assert_eq!(event.field("job_id"), Some(job.to_string()));
            assert_eq!(event.field("unique_key").as_deref(), Some("clip-1"));
            assert_eq!(event.field("error").as_deref(), Some("connection reset"));
        }
    }
}
