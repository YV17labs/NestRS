//! [`Leases`] — the delivery guard: which delivery of a job may run it, and
//! whether the job already reached its outcome.
//!
//! apalis-redis delivers a job **at least once**. Its fetch is exclusive, but a
//! replica's startup sweep puts every peer's in-flight jobs back on the queue
//! (it sweeps with a cutoff of *now*, which matches the living too), an
//! acknowledgement can be lost on its way to Redis, and a replica that stops
//! answering has its jobs handed to the others while it may still be running
//! them. Each of those is a second delivery of a job that is running or has
//! finished, and without a guard it runs again.
//!
//! So before an attempt runs, its delivery takes the job's **lease** —
//! `nestrs:queue:<queue>:leases:<job_id>`, `SET NX PX`, renewed while the
//! attempt runs — and when the job reaches its terminal outcome the delivery
//! writes its **settled** mark, `nestrs:queue:<queue>:settled:<job_id>`, and
//! drops the lease. A delivery that finds the mark acknowledges the job without
//! running it; one that finds the lease held elsewhere hands the job back for
//! when the lease would lapse, and never acknowledges it — the guard may delay a
//! job, never lose one. Every step is one Lua script, so no two deliveries ever
//! see the lease between a check and a write.
//!
//! **What it does not make exclusive** is a replica that stops renewing — a
//! network partition, a process frozen longer than the lease — while it goes on
//! running: once the lease lapses another delivery may take it. A lease is a
//! promise about time, and this one is kept for as long as its holder can reach
//! Redis. The line saying the holder lost it is an `error`.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_queue::{JobId, QueueName};
use redis::Script;
use tokio_util::task::AbortOnDropHandle;

use crate::RedisConnection;
use crate::layout::QUEUE_SLOT;

/// A delivery's claim on its job while an attempt runs, `{queue}` and `{job}`
/// standing for the queue's name and the job's id.
///
/// Levels of the queue's namespace ([`crate::layout`]), under a structure word
/// apalis does not use — `leases`, plural like every structure that holds one
/// member per job.
const LEASE: &str = "nestrs:queue:{queue}:leases:{job}";

/// The mark a job leaves when it reaches its terminal outcome, `{queue}` and
/// `{job}` standing for the queue's name and the job's id.
const SETTLED: &str = "nestrs:queue:{queue}:settled:{job}";

/// What [`LEASE`] and [`SETTLED`] write for the job's id.
const JOB_SLOT: &str = "{job}";

/// How long a settled job is remembered at the least: an hour, which covers a
/// second delivery waiting behind a backlog of that length — the case a startup
/// sweep creates, since it puts the job at the end of the queue.
const SETTLED_FLOOR: Duration = Duration::from_secs(60 * 60);

/// The admission step: settled ⇒ `{0, 0, how it settled}`; the lease taken ⇒
/// `{1, 0, ''}`; held by another delivery ⇒ `{2, ms until it lapses, its
/// holder}`. Always three members: a Lua table ends at its first `nil`.
const ADMIT: &str = r"
local settled = redis.call('GET', KEYS[1])
if settled then
  return {0, 0, settled}
end
if redis.call('SET', KEYS[2], ARGV[1], 'NX', 'PX', ARGV[2]) then
  return {1, 0, ''}
end
return {2, redis.call('PTTL', KEYS[2]), redis.call('GET', KEYS[2])}
";

/// Extend the lease while its holder still holds it: `1` renewed, `0` lost.
const RENEW: &str = r"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('PEXPIRE', KEYS[1], ARGV[2])
end
return 0
";

/// Mark the job settled, and drop the lease if this delivery still holds it.
const SETTLE: &str = r"
redis.call('SET', KEYS[1], ARGV[2], 'PX', ARGV[3])
if redis.call('GET', KEYS[2]) == ARGV[1] then
  redis.call('DEL', KEYS[2])
end
return 1
";

/// Keep a settled mark for longer, if it is still there: `1` extended, `0` gone.
const REMEMBER: &str = r"
if redis.call('EXISTS', KEYS[1]) == 1 then
  return redis.call('PEXPIRE', KEYS[1], ARGV[1])
end
return 0
";

/// Drop the lease if this delivery still holds it — an attempt that ends
/// without settling the job: a retry filed for later, a job handed back.
const RELEASE: &str = r"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('DEL', KEYS[1])
end
return 0
";

/// The delivery guard of one queue: its scripts, its connection and how long its
/// leases and settled marks last. One per worker, shared by its deliveries.
pub(crate) struct Leases {
    conn: RedisConnection,
    queue: QueueName,
    lease: Duration,
    settled_for: Duration,
    admit: Script,
    renew: Script,
    settle: Script,
    release: Script,
    remember: Script,
}

/// What a delivery may do with its job.
pub(crate) enum Admission {
    /// Run it: the lease is this delivery's.
    Granted(Lease),
    /// Answer it without running: the job already reached this outcome.
    Settled(Settlement),
    /// Hand it back: another delivery is running it, and its lease lapses in
    /// `lapses_in` unless renewed.
    Held {
        /// The delivery holding the lease, as it named itself.
        holder: String,
        /// How long until the lease lapses, if its holder stops renewing.
        lapses_in: Duration,
    },
}

/// How a job ended, as its settled mark records it.
#[derive(Clone, Copy)]
pub(crate) enum Settlement {
    /// The job completed.
    Completed,
    /// The job was dead-lettered.
    DeadLettered,
}

impl Settlement {
    /// The word the mark records, and a line reports.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::DeadLettered => "dead-lettered",
        }
    }

    /// The outcome a mark records. Anything but the dead-letter word reads as
    /// completed: a mark is only ever written after the job reached an outcome,
    /// and completion is the one answer that runs nothing and buries nothing.
    fn read(mark: &str) -> Self {
        if mark == Self::DeadLettered.as_str() {
            Self::DeadLettered
        } else {
            Self::Completed
        }
    }
}

impl Leases {
    /// The guard of `queue`, over `conn`: leases lasting `lease` past their last
    /// renewal, and settled marks outliving a second delivery a sweep after
    /// `orphan_after` could make.
    pub(crate) fn new(
        conn: RedisConnection,
        queue: QueueName,
        lease: Duration,
        orphan_after: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            conn,
            queue,
            lease,
            settled_for: settled_for(orphan_after, lease),
            admit: Script::new(ADMIT),
            renew: Script::new(RENEW),
            settle: Script::new(SETTLE),
            release: Script::new(RELEASE),
            remember: Script::new(REMEMBER),
        })
    }

    /// Ask whether the delivery `holder` may run `job`, taking the lease when
    /// it may.
    pub(crate) async fn admit(
        self: &Arc<Self>,
        job: &JobId,
        holder: String,
    ) -> Result<Admission, redis::RedisError> {
        let lease = key(LEASE, &self.queue, job);
        let answer: (i64, i64, String) = self
            .admit
            .key(key(SETTLED, &self.queue, job))
            .key(&lease)
            .arg(&holder)
            .arg(millis(self.lease))
            .invoke_async(&mut self.conn.clone())
            .await?;
        Ok(match answer {
            (0, _, mark) => Admission::Settled(Settlement::read(&mark)),
            (1, ..) => Admission::Granted(Lease {
                leases: Arc::clone(self),
                key: lease,
                job: job.clone(),
                holder,
            }),
            (_, lapses_in, holder) => Admission::Held {
                holder,
                lapses_in: Duration::from_millis(u64::try_from(lapses_in).unwrap_or(0)),
            },
        })
    }
}

impl Leases {
    /// Keep `job`'s settled mark for `longer` from now — a second delivery of a
    /// settled job, acknowledged while the worker drains, may lose that
    /// acknowledgement and come back after the mark would have lapsed.
    pub(crate) async fn remember(
        &self,
        job: &JobId,
        longer: Duration,
    ) -> Result<(), redis::RedisError> {
        self.remember
            .key(key(SETTLED, &self.queue, job))
            .arg(millis(longer.max(self.settled_for)))
            .invoke_async::<i64>(&mut self.conn.clone())
            .await
            .map(drop)
    }
}

/// A lease a delivery holds on its job while an attempt runs.
pub(crate) struct Lease {
    leases: Arc<Leases>,
    key: String,
    job: JobId,
    holder: String,
}

impl Lease {
    /// Keep the lease renewed until the handle is dropped: every third of its
    /// length, on a task of its own, so an attempt that blocks its thread does
    /// not starve the renewal. A renewal Redis refuses is said at `warn` and
    /// tried again; a lease found taken is said at `error` once, and no longer
    /// renewed.
    pub(crate) fn keep(&self) -> AbortOnDropHandle<()> {
        let leases = Arc::clone(&self.leases);
        let key = self.key.clone();
        let job = self.job.clone();
        let holder = self.holder.clone();
        AbortOnDropHandle::new(tokio::spawn(async move {
            let every = leases.lease / 3;
            loop {
                tokio::time::sleep(every).await;
                let renewed: Result<i64, _> = leases
                    .renew
                    .key(&key)
                    .arg(&holder)
                    .arg(millis(leases.lease))
                    .invoke_async(&mut leases.conn.clone())
                    .await;
                if !still_held(renewed, &leases.queue, &job, &holder, leases.lease) {
                    return;
                }
            }
        }))
    }

    /// Record that the job reached `outcome`, and drop the lease. The mark is
    /// kept for the guard's usual span, or for `remember` when the caller knows a
    /// second delivery may come later than any sweep would bring it.
    pub(crate) async fn settle(
        &self,
        outcome: Settlement,
        remember: Option<Duration>,
    ) -> Result<(), redis::RedisError> {
        let leases = &self.leases;
        let remember = remember.map_or(leases.settled_for, |longer| longer.max(leases.settled_for));
        leases
            .settle
            .key(key(SETTLED, &leases.queue, &self.job))
            .key(&self.key)
            .arg(&self.holder)
            .arg(outcome.as_str())
            .arg(millis(remember))
            .invoke_async::<i64>(&mut leases.conn.clone())
            .await
            .map(drop)
    }

    /// Drop the lease without settling the job: its next attempt is filed for
    /// later, or it was handed back.
    pub(crate) async fn release(&self) -> Result<(), redis::RedisError> {
        self.leases
            .release
            .key(&self.key)
            .arg(&self.holder)
            .invoke_async::<i64>(&mut self.leases.conn.clone())
            .await
            .map(drop)
    }
}

/// Whether a renewal left the lease with its holder, saying so when it did
/// not: a renewal Redis refused is said at `warn` and tried again at the next
/// beat; a lease found taken is said at `error` once, and no longer renewed.
fn still_held(
    renewed: Result<i64, redis::RedisError>,
    queue: &QueueName,
    job: &JobId,
    holder: &str,
    lease: Duration,
) -> bool {
    match renewed {
        Ok(1) => true,
        Ok(_) => {
            tracing::error!(
                target: nest_rs_queue::TARGET,
                queue = %queue,
                job_id = %job,
                holder,
                lease_ms = millis(lease),
                "job lease lost while its attempt runs; another delivery may run it too",
            );
            false
        }
        Err(error) => {
            tracing::warn!(
                target: nest_rs_queue::TARGET,
                queue = %queue,
                job_id = %job,
                holder,
                error = %nest_rs_core::error_message(&error),
                "job lease not renewed; retrying",
            );
            true
        }
    }
}

/// `template` for `job` on `queue`.
fn key(template: &str, queue: &QueueName, job: &JobId) -> String {
    template
        .replace(QUEUE_SLOT, queue.as_str())
        .replace(JOB_SLOT, &job.to_string())
}

/// How long a settled mark is kept: at least [`SETTLED_FLOOR`], and always past
/// the latest a sweep could hand the job to a second delivery — the orphan
/// threshold, then a lease left by the replica that was swept.
fn settled_for(orphan_after: Duration, lease: Duration) -> Duration {
    SETTLED_FLOOR.max(orphan_after.saturating_mul(2).saturating_add(lease))
}

/// `duration` in whole milliseconds, at least one — Redis refuses a zero `PX`.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis())
        .unwrap_or(u64::MAX)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio() -> QueueName {
        QueueName::new("audio").expect("a valid name")
    }

    /// Both keys are levels of the queue's namespace, under words apalis does
    /// not use — so a `SCAN` of the queue finds them, and none of apalis's
    /// structures is ever mistaken for one of the guard's.
    #[test]
    fn the_guards_keys_are_levels_of_the_queues_namespace() {
        let job = JobId::parse("01890a5d-ac96-774b-bcce-b302099a8057").expect("a job id");
        let namespace = crate::layout::namespace(&audio());
        assert_eq!(
            key(LEASE, &audio(), &job),
            format!("{namespace}:leases:{job}")
        );
        assert_eq!(
            key(SETTLED, &audio(), &job),
            format!("{namespace}:settled:{job}")
        );
    }

    /// A settled job is remembered past anything a sweep could redeliver it
    /// after, and never for less than the floor.
    #[test]
    fn a_settled_mark_outlives_every_redelivery_a_sweep_can_make() {
        let hour = Duration::from_secs(3600);
        assert_eq!(
            settled_for(Duration::from_secs(300), Duration::from_secs(30)),
            hour
        );
        let long = settled_for(hour, Duration::from_secs(30));
        assert!(long >= hour * 2 + Duration::from_secs(30), "{long:?}");
        assert_eq!(millis(Duration::ZERO), 1);
    }

    /// A renewal that finds the lease gone is the one guard failure that can run
    /// a job twice, so it is an `error` and ends the renewals; one Redis refused
    /// is a `warn`, and the next beat tries again.
    #[test]
    fn a_lease_found_taken_is_an_error_and_a_refused_renewal_is_retried() {
        let logs = nest_rs_testing::LogCapture::install();
        let job = JobId::parse("01890a5d-ac96-774b-bcce-b302099a8057").expect("a job id");
        let lease = Duration::from_secs(30);
        assert!(still_held(Ok(1), &audio(), &job, "host:01/a", lease));
        assert!(still_held(
            Err(redis::RedisError::from(std::io::Error::other("timed out"))),
            &audio(),
            &job,
            "host:01/a",
            lease,
        ));
        assert!(!still_held(Ok(0), &audio(), &job, "host:01/a", lease));

        let retried = logs.expect_one(nest_rs_queue::TARGET, "job lease not renewed; retrying");
        assert_eq!(retried.level, "warn");
        assert_eq!(retried.field("error").as_deref(), Some("timed out"));
        let lost = logs.expect_one(
            nest_rs_queue::TARGET,
            "job lease lost while its attempt runs; another delivery may run it too",
        );
        assert_eq!(lost.level, "error");
        assert_eq!(lost.field("holder").as_deref(), Some("host:01/a"));
        assert_eq!(lost.field("lease_ms").as_deref(), Some("30000"));
    }

    /// A mark reads back as the outcome it recorded, and only the dead-letter
    /// word reads as a dead letter.
    #[test]
    fn a_settled_mark_reads_back_as_the_outcome_it_recorded() {
        for outcome in [Settlement::Completed, Settlement::DeadLettered] {
            assert_eq!(
                Settlement::read(outcome.as_str()).as_str(),
                outcome.as_str()
            );
        }
        assert_eq!(Settlement::read("1").as_str(), "completed");
    }
}
