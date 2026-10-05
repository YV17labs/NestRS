//! Where a queue lives in Redis: the namespace every queue's records sit under,
//! shared by the producer that files them and the worker that runs them.
//!
//! **One namespace per queue, under the framework's prefix.** apalis derives a
//! queue's structures from the namespace it is handed — `<namespace>:active`,
//! `<namespace>:inflight:<worker>`, `<namespace>:scheduled`, `<namespace>:data`
//! and the rest — and this crate hands it `nestrs:queue:<queue>`, so everything a
//! queue holds is one `SCAN nestrs:queue:<queue>:*` away and nothing of it sits
//! at the root of the keyspace beside the application's own keys. The framework's
//! own records for a queue are levels of the same namespace, under words apalis
//! does not use — one structure per fact, keyed by the port's job id:
//!
//! | key | holds | from | until |
//! | --- | --- | --- | --- |
//! | `…:open:<job>` | the job's unique key, or nothing | its push | it settles, or is cancelled |
//! | `…:leases:<job>` | the delivery running it | an attempt starts | the attempt ends |
//! | `…:settled:<job>` | how it ended | its terminal outcome | a second delivery could no longer come |
//! | `…:cancelled:<job>` | the cancel | a cancel that promised it never starts | its delivery, and past it |
//! | `…:checkpoints:<job>` | its saved progress | a save | its terminal outcome |
//! | `…:attempts:<job>` | the attempts started and not handed back without an answer | its first attempt | its terminal outcome |
//! | `…:deferred:<job>` | when it was first handed back unread for a newer release | that hand-back | a delivery that reads it, or its terminal outcome |
//! | `…:unique:<key>` | the job holding the key | its push | it settles, or is cancelled |
//! | `…:throttle` | the attempts started in the current window | the window's first start | the window ends |
//!
//! **Nothing here waits forever.** A job can vanish without reaching an outcome
//! a delivery sees — a producer stopped between opening its records and filing
//! it, a replica that answered apalis in a way apalis buries on its own — so
//! every record of a job still waiting carries [`KEPT_PAST_DUE`] past the instant
//! the job is due, renewed whenever a delivery touches it. That is the bound on
//! a unique key whose job vanished, and on the records of a job nothing will
//! ever deliver.

use std::time::Duration;

use apalis_redis::Config;
use nest_rs_queue::{JobId, QueueName};

/// The namespace apalis files one queue's structures under, `{queue}` standing
/// for the queue's name.
///
/// `nestrs:<concern>:<member>`: the concern is the tail of
/// [`nest_rs_queue::TARGET`] — the port that owns queues, never `redis` — and the
/// member is the queue, one level, since a queue name holds no `:`. Fixed rather
/// than the deployment's: `NESTRS_ENV_PREFIX` renames the developer's variables,
/// while a key is the framework's machinery, and two deployments sharing a Redis
/// are separated by the logical database in the URL.
const NAMESPACE: &str = "nestrs:queue:{queue}";

/// What [`NAMESPACE`] and the keys built beside it write for the queue's name.
const QUEUE_SLOT: &str = "{queue}";

/// What a job's keys write for the job's id — the port's [`JobId`], never
/// apalis's own task id.
const JOB_SLOT: &str = "{job}";

/// What a unique claim's key writes for the key the push declared.
const KEY_SLOT: &str = "{key}";

/// A job that has not reached its outcome, holding the unique key it was pushed
/// under (empty when it has none): opened by the push, closed when the job
/// settles or is cancelled. What lets a cancel tell a job still waiting from one
/// that finished long ago — apalis keeps a finished job's record until someone
/// vacuums it.
pub(crate) const OPEN: &str = "nestrs:queue:{queue}:open:{job}";

/// A delivery's claim on its job while an attempt runs — plural like every
/// structure that holds one member per job.
pub(crate) const LEASES: &str = "nestrs:queue:{queue}:leases:{job}";

/// The mark a job leaves when it reaches its terminal outcome.
pub(crate) const SETTLED: &str = "nestrs:queue:{queue}:settled:{job}";

/// A cancel's promise that the job never starts: whichever delivery meets it
/// acknowledges the job without running it.
pub(crate) const CANCELLED: &str = "nestrs:queue:{queue}:cancelled:{job}";

/// The progress a job saved through its `Checkpoint`.
pub(crate) const CHECKPOINTS: &str = "nestrs:queue:{queue}:checkpoints:{job}";

/// How many attempts at a job have started and not been handed back without
/// an answer —
/// the count the port reads an attempt that never returned from, since the
/// envelope only counts the ones that answered.
pub(crate) const ATTEMPTS: &str = "nestrs:queue:{queue}:attempts:{job}";

/// When a job was first handed back unread because a newer release sealed it —
/// Redis's own millisecond — for as long as every delivery since has handed it
/// back so: how long it has waited for a consumer of that release, which the
/// port bounds, counted without charging a delayed push for its delay.
pub(crate) const DEFERRED: &str = "nestrs:queue:{queue}:deferred:{job}";

/// The job holding a unique key on the queue.
const UNIQUE: &str = "nestrs:queue:{queue}:unique:{key}";

/// The attempts of the queue's method started in the current throttle window.
/// One per queue: a queue is drained by one `#[process]` method, so the queue
/// already names the method a throttle belongs to.
const THROTTLE: &str = "nestrs:queue:{queue}:throttle";

/// How long a job's records outlive the instant the job is due while nothing
/// touches them: a week — the quiet a deployment scaled to zero may sit
/// through. Every delivery renews it; a job still waiting a week past its due
/// time has been forgotten, and its unique key is free again.
pub(crate) const KEPT_PAST_DUE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The namespace `queue`'s records live under — apalis's structures and the
/// framework's own.
pub(crate) fn namespace(queue: &QueueName) -> String {
    NAMESPACE.replace(QUEUE_SLOT, queue.as_str())
}

/// The key `template` names for `job` on `queue` — one of [`OPEN`], [`LEASES`],
/// [`SETTLED`], [`CANCELLED`], [`CHECKPOINTS`], [`ATTEMPTS`] or [`DEFERRED`].
pub(crate) fn job_key(template: &str, queue: &QueueName, job: &JobId) -> String {
    template
        .replace(QUEUE_SLOT, queue.as_str())
        .replace(JOB_SLOT, &job.to_string())
}

/// The key the job holding the unique key `key` on `queue` is named under. The
/// key is the caller's, written last so nothing it spells is read as a slot.
pub(crate) fn unique_key(queue: &QueueName, key: &str) -> String {
    UNIQUE
        .replace(QUEUE_SLOT, queue.as_str())
        .replace(KEY_SLOT, key)
}

/// The key `queue`'s throttle window is counted in.
pub(crate) fn throttle_key(queue: &QueueName) -> String {
    THROTTLE.replace(QUEUE_SLOT, queue.as_str())
}

/// `duration` in whole milliseconds, at least one — Redis refuses a zero `PX` —
/// and at most [`Delay::LATEST_DUE`](nest_rs_queue::Delay::LATEST_DUE): Redis
/// refuses an expiry whose instant overflows its `i64` milliseconds, and a
/// duration a declaration may carry — a throttle window of `u64::MAX` ms —
/// reached it, failing every admission. Past that bound a key outlives any
/// deployment, which is what a longer one would have meant.
pub(crate) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.min(nest_rs_queue::Delay::LATEST_DUE).as_millis())
        .unwrap_or(u64::MAX)
        .max(1)
}

/// The storage settings every handle on `queue` starts from: its namespace. A
/// producer uses them as they are; the worker adds its fetch and liveness
/// settings on top.
pub(crate) fn config(queue: &QueueName) -> Config {
    Config::default().set_namespace(&namespace(queue))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every duration a key is kept for is one Redis accepts: a window a
    /// declaration may carry up to `u64::MAX` ms used to reach `PEXPIRE`
    /// whole, which Redis refuses.
    #[test]
    fn a_duration_redis_would_refuse_is_kept_for_the_longest_it_accepts() {
        let longest =
            u64::try_from(nest_rs_queue::Delay::LATEST_DUE.as_millis()).expect("the bound fits");
        assert_eq!(millis(Duration::MAX), longest);
        assert_eq!(millis(Duration::from_millis(u64::MAX)), longest);
        assert!(i64::try_from(longest).is_ok_and(|ms| ms < i64::MAX / 2));
        assert_eq!(millis(Duration::from_micros(500)), 1, "never zero");
    }

    fn audio() -> QueueName {
        QueueName::new("audio").expect("a valid name")
    }

    /// Every key this crate writes, beside the span target of the crate that
    /// owns its concern — the queue's, the scheduler's, the rate limiter's,
    /// never `redis`.
    fn every_key() -> Vec<(&'static str, &'static str)> {
        let queue = [
            NAMESPACE,
            OPEN,
            LEASES,
            SETTLED,
            CANCELLED,
            CHECKPOINTS,
            ATTEMPTS,
            DEFERRED,
            UNIQUE,
            THROTTLE,
        ];
        let mut keys: Vec<_> = queue.map(|key| (key, nest_rs_queue::TARGET)).into();
        #[cfg(feature = "schedule")]
        keys.push((crate::schedule::CLAIMS, nest_rs_schedule::TARGET));
        #[cfg(feature = "throttler")]
        keys.push((crate::throttler::BUCKETS, nest_rs_throttler::TARGET));
        keys
    }

    /// Every key is `nestrs:<concern>:<structure>[:…]`, its concern read off
    /// the owning crate's span target — so renaming the target moves the keys,
    /// or fails here — a queue's member-first, and no key another's twin or its
    /// prefix but at a level, so a `SCAN` of one never matches the other.
    #[test]
    fn every_key_is_a_level_of_the_concern_its_owner_emits_on() {
        let keys = every_key();
        for (at, &(key, target)) in keys.iter().enumerate() {
            let concern = target
                .strip_prefix("nest_rs::")
                .expect("a framework target")
                .replace("::", ":");
            let levels: Vec<_> = key.split(':').collect();
            assert!(
                key.starts_with(&format!("nestrs:{concern}:")) && levels.len() >= 3,
                "{key} is `nestrs:{concern}:<structure>`",
            );
            if target == nest_rs_queue::TARGET {
                assert_eq!(levels[2], QUEUE_SLOT, "{key} names its queue first");
            }
            for (other_at, &(other, _)) in keys.iter().enumerate() {
                if other_at != at
                    && let Some(rest) = other.strip_prefix(key)
                {
                    assert!(
                        rest.starts_with(':'),
                        "{key} prefixes {other} inside a level"
                    );
                }
            }
        }
        assert_eq!(config(&audio()).get_namespace(), &namespace(&audio()));
    }

    /// Every record of a job is a level of its queue's namespace, under a word
    /// apalis does not use — so a `SCAN` of the queue finds them, and none of
    /// apalis's structures is ever mistaken for one of the framework's.
    #[test]
    fn a_jobs_records_are_levels_of_its_queues_namespace() {
        let job = JobId::parse("01890a5d-ac96-774b-bcce-b302099a8057").expect("a job id");
        let namespace = namespace(&audio());
        for (template, word) in [
            (OPEN, "open"),
            (LEASES, "leases"),
            (SETTLED, "settled"),
            (CANCELLED, "cancelled"),
            (CHECKPOINTS, "checkpoints"),
            (ATTEMPTS, "attempts"),
            (DEFERRED, "deferred"),
        ] {
            assert_eq!(
                job_key(template, &audio(), &job),
                format!("{namespace}:{word}:{job}")
            );
        }
        assert_eq!(
            unique_key(&audio(), "clip:{job}"),
            format!("{namespace}:unique:clip:{{job}}"),
            "the caller's key is written as it came, slots and levels included",
        );
        assert_eq!(throttle_key(&audio()), format!("{namespace}:throttle"));
        assert_eq!(millis(Duration::ZERO), 1);
    }
}
