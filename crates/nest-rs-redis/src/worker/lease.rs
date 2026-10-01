//! [`Leases`] — the delivery guard: which delivery of a job may run it, whether
//! the job already reached its outcome or was cancelled before it started, and
//! whether its method's throttle lets another attempt start now.
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
//! `nestrs:queue:<queue>:leases:<job_id>`, renewed while the attempt runs — and
//! when the job reaches its terminal outcome the delivery writes its **settled**
//! mark, `nestrs:queue:<queue>:settled:<job_id>`, and drops the lease. A delivery
//! that finds the mark acknowledges the job without running it; one that finds
//! the lease held elsewhere hands the job back for when the lease would lapse,
//! and never acknowledges it — the guard may delay a job, never lose one.
//!
//! **The same step answers the capabilities that act before an attempt.** A job
//! a cancel reached first carries its **tombstone**, and the delivery meeting it
//! acknowledges the job without running it — the cancel's promise, kept on
//! whichever replica the job reaches. A method declaring a throttle counts the
//! attempts it starts in its queue's **window**, and an attempt over the limit is
//! handed back for when the window ends, neither counted nor dropped. Settling a
//! job closes what it held: its open record and its unique key. Every step is one
//! Lua script, so no cancel, no second delivery and no other replica's start ever
//! sees the lease between a check and a write.
//!
//! **The guard filters duplicates; it does not make delivery exactly once.** The
//! queue's contract is at least once, so a handler is idempotent. A settled mark
//! lasts one fixed span — past the latest a sweep could hand the job to a second
//! delivery ([`settled_for`]) — and a delivery arriving after it lapsed runs the
//! job again. apalis-redis 0.7 acknowledges a job from its worker's own loop,
//! after the delivery has answered, and drops every acknowledgement still queued
//! when the worker stops, and one Redis refuses: the job then waits in that
//! replica's flight until a replica starts and sweeps it, which may be after its
//! mark is gone.
//!
//! **What it does not make exclusive** is a replica that stops renewing — a
//! network partition, a process frozen longer than the lease — while it goes on
//! running: once the lease lapses another delivery may take it. A lease is a
//! promise about time, and this one is kept for as long as its holder can reach
//! Redis. The line saying the holder lost it is an `error`.

use std::sync::Arc;
use std::time::Duration;

use nest_rs_queue::{JobId, QueueName, Throttle};
use redis::Script;
use tokio_util::task::AbortOnDropHandle;

use crate::RedisConnection;
use crate::layout::{
    self, ATTEMPTS, CANCELLED, CHECKPOINTS, DEFERRED, KEPT_PAST_DUE, LEASES, OPEN, SETTLED,
    job_key, millis,
};

/// How long a settled job is remembered at the least: an hour, which covers a
/// second delivery waiting behind a backlog of that length — the case a startup
/// sweep creates, since it puts the job at the end of the queue.
const SETTLED_FLOOR: Duration = Duration::from_secs(60 * 60);

/// The admission step, in the order a delivery owes its answers:
///
/// | answer | reply |
/// | --- | --- |
/// | settled | `{0, 0, how it settled, ''}` |
/// | the lease taken | `{1, attempts started with this one, the throttle window's end or '', ms it has waited unread or ''}` |
/// | held by another delivery | `{2, ms until it lapses, its holder, ''}` |
/// | cancelled | `{3, 0, '', ''}` |
/// | over its throttle | `{4, ms until the window ends, '', ''}` |
///
/// Always four members: a Lua table ends at its first `nil`.
///
/// `KEYS`: settled, lease, cancelled, open, checkpoints, throttle, attempts,
/// deferred, then the unique claim when the job holds one. `ARGV`: the holder,
/// the lease's length, how long the job's records are kept, the job's id, the
/// throttle's limit (`0` for none) and its window.
///
/// How long a job granted has waited unread is read off its deferred record,
/// both instants Redis's own, so no two hosts' clocks are compared.
///
/// A job met cancelled lets go of what it held and keeps its tombstone for as
/// long as a delivery of it could come again. A job granted counts the attempt
/// it starts; one granted or deferred renews what it holds by as long, counted
/// from its next delivery.
///
/// A start counted against the throttle answers the instant its window ends,
/// in Redis's own milliseconds — the window's identity, which [`RELEASE`]
/// compares before it takes the start back: the counter is one key, and a
/// window that ended between the two scripts is another window's count.
const ADMIT: &str = r"
local settled = redis.call('GET', KEYS[1])
if settled then
  return {0, 0, settled, ''}
end
if redis.call('EXISTS', KEYS[3]) == 1 then
  redis.call('PEXPIRE', KEYS[3], ARGV[3])
  redis.call('DEL', KEYS[4], KEYS[5], KEYS[7], KEYS[8])
  if KEYS[9] and redis.call('GET', KEYS[9]) == ARGV[4] then
    redis.call('DEL', KEYS[9])
  end
  return {3, 0, '', ''}
end
local holder = redis.call('GET', KEYS[2])
if holder then
  return {2, redis.call('PTTL', KEYS[2]), holder, ''}
end
local limit = tonumber(ARGV[5])
if limit > 0 then
  local started = tonumber(redis.call('GET', KEYS[6]) or '0')
  local ends = redis.call('PTTL', KEYS[6])
  if started >= limit then
    if ends < 0 then
      redis.call('PEXPIRE', KEYS[6], ARGV[6])
      ends = tonumber(ARGV[6])
    end
    local kept = tonumber(ARGV[3]) + ends
    redis.call('PEXPIRE', KEYS[4], kept)
    redis.call('PEXPIRE', KEYS[5], kept)
    redis.call('PEXPIRE', KEYS[7], kept)
    redis.call('PEXPIRE', KEYS[8], kept)
    if KEYS[9] and redis.call('GET', KEYS[9]) == ARGV[4] then
      redis.call('PEXPIRE', KEYS[9], kept)
    end
    return {4, ends, '', ''}
  end
  redis.call('INCR', KEYS[6])
  if ends < 0 then
    redis.call('PEXPIRE', KEYS[6], ARGV[6])
  end
end
redis.call('SET', KEYS[2], ARGV[1], 'PX', ARGV[2])
local started = redis.call('INCR', KEYS[7])
redis.call('PEXPIRE', KEYS[7], ARGV[3])
redis.call('PEXPIRE', KEYS[4], ARGV[3])
redis.call('PEXPIRE', KEYS[5], ARGV[3])
redis.call('PEXPIRE', KEYS[8], ARGV[3])
if KEYS[9] and redis.call('GET', KEYS[9]) == ARGV[4] then
  redis.call('PEXPIRE', KEYS[9], ARGV[3])
end
local now = redis.call('TIME')
now = tonumber(now[1]) * 1000 + math.floor(tonumber(now[2]) / 1000)
local window = ''
if limit > 0 then
  window = tostring(now + redis.call('PTTL', KEYS[6]))
end
local waited = ''
local since = tonumber(redis.call('GET', KEYS[8]) or '')
if since then
  waited = tostring(math.max(0, now - since))
end
return {1, started, window, waited}
";

/// Extend the lease while its holder still holds it: `1` renewed, `0` lost.
const RENEW: &str = r"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('PEXPIRE', KEYS[1], ARGV[2])
end
return 0
";

/// Mark the job settled, drop the lease if this delivery still holds it, and
/// close what the job held: its open record, its count of attempts, its
/// deferral record, and its unique key if it is still the job's.
///
/// `KEYS`: settled, lease, open, attempts, deferred, then the unique claim when
/// the job holds one. `ARGV`: the holder, the outcome, how long the mark is
/// kept, the job's id.
const SETTLE: &str = r"
redis.call('SET', KEYS[1], ARGV[2], 'PX', ARGV[3])
if redis.call('GET', KEYS[2]) == ARGV[1] then
  redis.call('DEL', KEYS[2])
end
redis.call('DEL', KEYS[3], KEYS[4], KEYS[5])
if KEYS[6] and redis.call('GET', KEYS[6]) == ARGV[4] then
  redis.call('DEL', KEYS[6])
end
return 1
";

/// Drop the lease if this delivery still holds it — an attempt that ends
/// without settling the job: a retry filed for later, a job handed back — and
/// renew what the job holds until its next delivery and past it. What the
/// attempt gives back ([`Unsettled`]) is taken back while its lease is still its
/// own: past that, the count may be another delivery's. The throttle's start is
/// taken back only inside the window that counted it — the one ending at the
/// instant [`ADMIT`] answered, give or take the millisecond two clock readings
/// can differ by. The job's deferral record is the unread attempt's to start —
/// at the first of an unbroken run of hand-backs unread, in Redis's own
/// millisecond — and any other attempt's to clear, since a delivery that ran
/// the job could read it.
///
/// `KEYS`: lease, open, checkpoints, attempts, throttle, deferred, then the
/// unique claim when the job holds one. `ARGV`: the holder, how long the job's
/// records are kept from now, the job's id, what the attempt gives back (`0`
/// nothing, `1` its attempt, `2` its attempt and its throttle start — the
/// attempt that did not read the job), the end of the window its throttle start
/// was counted in (`''` for none), and how far apart two readings of that end
/// may be and still name one window.
const RELEASE: &str = r"
local released = 0
if redis.call('GET', KEYS[1]) == ARGV[1] then
  released = redis.call('DEL', KEYS[1])
  if ARGV[4] ~= '0' and tonumber(redis.call('GET', KEYS[4]) or '0') > 0 then
    redis.call('DECR', KEYS[4])
  end
  local now = redis.call('TIME')
  now = tonumber(now[1]) * 1000 + math.floor(tonumber(now[2]) / 1000)
  if ARGV[4] == '2' and ARGV[5] ~= '' then
    local left = redis.call('PTTL', KEYS[5])
    if left > 0 and tonumber(redis.call('GET', KEYS[5]) or '0') > 0 then
      if math.abs(now + left - tonumber(ARGV[5])) <= tonumber(ARGV[6]) then
        redis.call('DECR', KEYS[5])
      end
    end
  end
  if ARGV[4] == '2' then
    if redis.call('EXISTS', KEYS[6]) == 0 then
      redis.call('SET', KEYS[6], tostring(now))
    end
  else
    redis.call('DEL', KEYS[6])
  end
end
redis.call('PEXPIRE', KEYS[2], ARGV[2])
redis.call('PEXPIRE', KEYS[3], ARGV[2])
redis.call('PEXPIRE', KEYS[4], ARGV[2])
redis.call('PEXPIRE', KEYS[6], ARGV[2])
if KEYS[7] and redis.call('GET', KEYS[7]) == ARGV[3] then
  redis.call('PEXPIRE', KEYS[7], ARGV[2])
end
return released
";

/// The delivery guard of one queue: its scripts, its connection, how long its
/// leases and settled marks last, and the throttle its method declares. One per
/// worker, shared by its deliveries.
pub(crate) struct Leases {
    conn: RedisConnection,
    /// The same connection without its budget, which admission runs on: a cut
    /// admission still runs, and would leave a lease, a throttle start and an
    /// attempt no delivery holds.
    admitting: RedisConnection,
    queue: QueueName,
    lease: Duration,
    settled_for: Duration,
    throttle: Option<Throttle>,
    admit: Script,
    renew: Script,
    settle: Script,
    release: Script,
}

/// What a delivery may do with its job.
pub(crate) enum Admission {
    /// Run it: the lease is this delivery's.
    Granted(Lease),
    /// Answer it without running: the job already reached this outcome.
    Settled(Settlement),
    /// Answer it without running: a cancel reached it before any attempt
    /// started, and promised it never would.
    Cancelled,
    /// Hand it back: another delivery is running it, and its lease lapses in
    /// `lapses_in` unless renewed.
    Held {
        /// The delivery holding the lease, as it named itself.
        holder: String,
        /// How long until the lease lapses, if its holder stops renewing.
        lapses_in: Duration,
    },
    /// Hand it back: its method started as many attempts as its throttle allows
    /// in the current window, which ends in `ends_in`. Not an attempt: nothing
    /// was counted.
    Throttled {
        /// How long until the window ends.
        ends_in: Duration,
    },
}

/// What an attempt that goes back to the queue gives back of what its
/// admission counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unsettled {
    /// It answered — a retry filed for later: its start counts against the
    /// budget and the throttle alike.
    Answered,
    /// It was cut before it answered — the drain window closed on it: the
    /// budget takes its start back, since the attempt never returned, and the
    /// throttle keeps it, since the attempt ran and did whatever it did
    /// downstream before the cut.
    Cut,
    /// Nothing ran — a job a newer release sealed: the budget and the throttle
    /// both take its start back, since the method was never called.
    Unread,
}

impl Unsettled {
    /// `ARGV[4]` of [`RELEASE`].
    fn gives_back(self) -> u8 {
        match self {
            Self::Answered => 0,
            Self::Cut => 1,
            Self::Unread => 2,
        }
    }

    /// What it gives back, for a line saying it was not.
    pub(crate) fn gives_back_in_words(self) -> &'static str {
        match self {
            Self::Answered => "nothing",
            Self::Cut => "its attempt",
            Self::Unread => "its attempt and its throttle start",
        }
    }
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
    /// renewal, settled marks outliving a second delivery a sweep after
    /// `orphan_after` could make, and the starts `throttle` allows, if any.
    pub(crate) fn new(
        conn: RedisConnection,
        queue: QueueName,
        lease: Duration,
        orphan_after: Duration,
        throttle: Option<Throttle>,
    ) -> Arc<Self> {
        Arc::new(Self {
            admitting: conn.without_budget(),
            conn,
            queue,
            lease,
            settled_for: settled_for(orphan_after, lease),
            throttle,
            admit: Script::new(ADMIT),
            renew: Script::new(RENEW),
            settle: Script::new(SETTLE),
            release: Script::new(RELEASE),
        })
    }

    /// Ask whether the delivery `holder` may run `job`, which holds the unique
    /// key `unique` if it was pushed under one — taking the lease, and counting
    /// the start against the throttle, when it may.
    pub(crate) async fn admit(
        self: &Arc<Self>,
        job: &JobId,
        unique: Option<&str>,
        holder: String,
    ) -> Result<Admission, redis::RedisError> {
        let lease = job_key(LEASES, &self.queue, job);
        let claim = unique.map(|key| layout::unique_key(&self.queue, key));
        let (limit, window) = self.throttle.map_or((0, 0), |throttle| {
            (throttle.limit().get(), millis(throttle.window()))
        });
        let mut invocation = self.admit.key(job_key(SETTLED, &self.queue, job));
        invocation
            .key(&lease)
            .key(job_key(CANCELLED, &self.queue, job))
            .key(job_key(OPEN, &self.queue, job))
            .key(job_key(CHECKPOINTS, &self.queue, job))
            .key(layout::throttle_key(&self.queue))
            .key(job_key(ATTEMPTS, &self.queue, job))
            .key(job_key(DEFERRED, &self.queue, job));
        if let Some(claim) = &claim {
            invocation.key(claim);
        }
        let answer: (i64, i64, String, String) = invocation
            .arg(&holder)
            .arg(millis(self.lease))
            .arg(millis(KEPT_PAST_DUE))
            .arg(job.to_string())
            .arg(limit)
            .arg(window)
            .invoke_async(&mut self.admitting.clone())
            .await?;
        Ok(match answer {
            (0, _, mark, _) => Admission::Settled(Settlement::read(&mark)),
            (1, started, window, waited) => Admission::Granted(Lease {
                leases: Arc::clone(self),
                key: lease,
                claim,
                job: job.clone(),
                holder,
                started: u32::try_from(started).unwrap_or(u32::MAX),
                window,
                deferred_for: Duration::from_millis(waited.parse().unwrap_or(0)),
            }),
            (3, ..) => Admission::Cancelled,
            (4, ends_in, ..) => Admission::Throttled {
                ends_in: Duration::from_millis(u64::try_from(ends_in).unwrap_or(0)),
            },
            (_, lapses_in, holder, _) => Admission::Held {
                holder,
                lapses_in: Duration::from_millis(u64::try_from(lapses_in).unwrap_or(0)),
            },
        })
    }
}

/// A lease a delivery holds on its job while an attempt runs.
pub(crate) struct Lease {
    leases: Arc<Leases>,
    key: String,
    /// The key of the unique claim the job holds, if it was pushed under one.
    claim: Option<String>,
    job: JobId,
    holder: String,
    /// How many attempts at the job have started, this one included.
    started: u32,
    /// When the throttle window this attempt's start was counted in ends, in
    /// Redis's milliseconds as [`ADMIT`] answered it — `""` when the method
    /// declares no throttle. Handed back to [`RELEASE`] as it came.
    window: String,
    /// How long the job has been handed back unread for a newer release, since
    /// the first of an unbroken run of such hand-backs — zero for none.
    deferred_for: Duration,
}

impl Lease {
    /// How many attempts at the job have started, the one this lease runs
    /// included — the port reads an attempt that never returned from it.
    pub(crate) fn started(&self) -> u32 {
        self.started
    }

    /// How long the job has waited handed back unread for a newer release —
    /// the wait the port bounds.
    pub(crate) fn deferred_for(&self) -> Duration {
        self.deferred_for
    }

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

    /// Record that the job reached `outcome`, drop the lease, and close what the
    /// job held. The mark is kept for the guard's one span ([`settled_for`]).
    pub(crate) async fn settle(&self, outcome: Settlement) -> Result<(), redis::RedisError> {
        let leases = &self.leases;
        let mut invocation = leases
            .settle
            .key(job_key(SETTLED, &leases.queue, &self.job));
        invocation
            .key(&self.key)
            .key(job_key(OPEN, &leases.queue, &self.job))
            .key(job_key(ATTEMPTS, &leases.queue, &self.job))
            .key(job_key(DEFERRED, &leases.queue, &self.job));
        if let Some(claim) = &self.claim {
            invocation.key(claim);
        }
        invocation
            .arg(&self.holder)
            .arg(outcome.as_str())
            .arg(millis(leases.settled_for))
            .arg(self.job.to_string())
            .invoke_async::<i64>(&mut leases.conn.clone())
            .await
            .map(drop)
    }

    /// Drop the lease without settling the job: its next attempt is filed for
    /// `next` from now, or it was handed back — and take back what `unsettled`
    /// says the attempt did not spend. What the job holds is renewed until that
    /// delivery and past it.
    pub(crate) async fn release(
        &self,
        next: Duration,
        unsettled: Unsettled,
    ) -> Result<(), redis::RedisError> {
        let leases = &self.leases;
        let mut invocation = leases.release.key(&self.key);
        invocation
            .key(job_key(OPEN, &leases.queue, &self.job))
            .key(job_key(CHECKPOINTS, &leases.queue, &self.job))
            .key(job_key(ATTEMPTS, &leases.queue, &self.job))
            .key(layout::throttle_key(&leases.queue))
            .key(job_key(DEFERRED, &leases.queue, &self.job));
        if let Some(claim) = &self.claim {
            invocation.key(claim);
        }
        invocation
            .arg(&self.holder)
            .arg(millis(next.saturating_add(KEPT_PAST_DUE)))
            .arg(self.job.to_string())
            .arg(unsettled.gives_back())
            .arg(&self.window)
            .arg(same_window_within(leases.throttle))
            .invoke_async::<i64>(&mut leases.conn.clone())
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

/// How far apart, in milliseconds, two readings of one throttle window's end
/// may be and still name that window: one, the most a clock reading and a
/// `PTTL` truncated to the millisecond can differ by — and never as far as the
/// next window's end, which is a whole window later, so a one-millisecond
/// window must match exactly. A start this misses stays counted, which errs
/// towards starting fewer.
fn same_window_within(throttle: Option<Throttle>) -> u64 {
    throttle.map_or(0, |throttle| {
        millis(throttle.window()).saturating_sub(1).min(1)
    })
}

/// How long a settled mark is kept: at least [`SETTLED_FLOOR`], and always past
/// the latest a sweep could hand the job to a second delivery — the orphan
/// threshold, then a lease left by the replica that was swept. One span for
/// every mark, whenever it is written: a redelivery later than it — an
/// acknowledgement apalis dropped, then a quiet longer than the span before a
/// replica starts — runs the job again, which at least once allows.
fn settled_for(orphan_after: Duration, lease: Duration) -> Duration {
    SETTLED_FLOOR.max(orphan_after.saturating_mul(2).saturating_add(lease))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio() -> QueueName {
        QueueName::new("audio").expect("a valid name")
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
    }

    /// A renewal that finds the lease gone means another delivery may run the
    /// job beside this attempt, so it is an `error` and ends the renewals; one
    /// Redis refused is a `warn`, and the next beat tries again.
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
