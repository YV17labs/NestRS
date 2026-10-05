//! Where a queue lives in Redis: the keys every queue's records sit under,
//! shared by the producer that files them and the worker that runs them.
//!
//! **One queue, one hash slot.** Every key of a queue is
//! `nestrs:queue:{<queue>}:<structure>`: the braces are a hash tag, so a
//! queue's keys sit on one Redis Cluster node and every script names each key it
//! touches. A fact about one job is a field of a per-queue hash, keyed by the
//! port's job id, never a key of its own:
//!
//! | key | type | holds | from | until |
//! | --- | --- | --- | --- | --- |
//! | `…:jobs` | stream | one entry per job ready to run or running; consumer group `workers` | its filing | its delivery ends |
//! | `…:entries` | hash | job → its entry in `jobs` | its filing | its delivery ends |
//! | `…:due` | sorted set | job → when it is due, in Redis's milliseconds | a delayed filing | it is due |
//! | `…:delayed` | hash | job → its record, while it waits to be due | a delayed filing | it is due |
//! | `…:unique` | hash | unique key → the job holding it | its push | the job ends |
//! | `…:claims` | hash | job → the unique key it holds | its push | the job ends |
//! | `…:deferred` | hash | job → when a newer release's job was first handed back unread | that hand-back | a delivery that reads it |
//! | `…:checkpoints` | hash | job → its saved progress | a save | the job ends |
//! | `…:throttle` | string | the starts counted in the current window | the window's first start | the window ends |
//! | `…:dead` | stream | one entry per dead letter: `job`, `record`, `reason` | the job dead-letters | 7 days or 10,000 entries |
//!
//! **No record outlives its job.** Every transition is one script that writes
//! what the job's next state needs and removes what it no longer does, so a job
//! that ended leaves nothing but its dead letter, and nothing needs an expiry
//! to bound it.

use std::time::Duration;

use nest_rs_queue::QueueName;

/// The stream a queue's jobs wait and run on.
const JOBS: &str = "nestrs:queue:{}:jobs";

/// Each job in [`JOBS`], by the id of its entry there.
const ENTRIES: &str = "nestrs:queue:{}:entries";

/// When each delayed job is due.
const DUE: &str = "nestrs:queue:{}:due";

/// The record of each delayed job.
const DELAYED: &str = "nestrs:queue:{}:delayed";

/// The job holding each unique key.
const UNIQUE: &str = "nestrs:queue:{}:unique";

/// The unique key each job holds.
const CLAIMS: &str = "nestrs:queue:{}:claims";

/// When a job a newer release sealed was first handed back unread.
const DEFERRED: &str = "nestrs:queue:{}:deferred";

/// The progress each job saved.
const CHECKPOINTS: &str = "nestrs:queue:{}:checkpoints";

/// The starts of the queue's method in the current throttle window. One per
/// queue: a queue is drained by one `#[process]` method, so the queue names the
/// method a throttle belongs to.
const THROTTLE: &str = "nestrs:queue:{}:throttle";

/// The queue's dead letters.
const DEAD: &str = "nestrs:queue:{}:dead";

/// Where each key template takes the queue's name, inside the hash tag.
const HASH_TAG: &str = "{}";

/// The consumer group every worker of a queue reads [`JOBS`] through.
pub(crate) const GROUP: &str = "workers";

/// How long a dead letter is kept, at the least.
pub(crate) const DEAD_KEPT: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How many dead letters a queue keeps, at the most — about: Redis trims a
/// stream by whole nodes, so a few more may stay.
pub(crate) const DEAD_MOST: u32 = 10_000;

/// Every key of one queue, spelled once — so a push or a settle builds none.
#[derive(Clone, Debug)]
pub(crate) struct QueueKeys {
    pub(crate) jobs: String,
    pub(crate) entries: String,
    pub(crate) due: String,
    pub(crate) delayed: String,
    pub(crate) unique: String,
    pub(crate) claims: String,
    pub(crate) deferred: String,
    pub(crate) checkpoints: String,
    pub(crate) throttle: String,
    pub(crate) dead: String,
}

impl QueueKeys {
    /// The keys of `queue`.
    pub(crate) fn new(queue: &QueueName) -> Self {
        let tag = format!("{{{queue}}}");
        let key = |template: &str| template.replacen(HASH_TAG, &tag, 1);
        Self {
            jobs: key(JOBS),
            entries: key(ENTRIES),
            due: key(DUE),
            delayed: key(DELAYED),
            unique: key(UNIQUE),
            claims: key(CLAIMS),
            deferred: key(DEFERRED),
            checkpoints: key(CHECKPOINTS),
            throttle: key(THROTTLE),
            dead: key(DEAD),
        }
    }
}

/// `duration` in whole milliseconds, at least one — Redis refuses a zero `PX` —
/// and at most [`Delay::LATEST_DUE`](nest_rs_queue::Delay::LATEST_DUE): a
/// duration a declaration may carry — a throttle window of `u64::MAX` ms — would
/// overflow the instant Redis computes from it. Past that bound a key outlives
/// any deployment, which is what a longer one would have meant.
pub(crate) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.min(nest_rs_queue::Delay::LATEST_DUE).as_millis())
        .unwrap_or(u64::MAX)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every duration a key is kept for is one Redis accepts.
    #[test]
    fn a_duration_redis_would_refuse_is_kept_for_the_longest_it_accepts() {
        let longest =
            u64::try_from(nest_rs_queue::Delay::LATEST_DUE.as_millis()).expect("the bound fits");
        assert_eq!(millis(Duration::MAX), longest);
        assert_eq!(millis(Duration::from_millis(u64::MAX)), longest);
        assert!(longest < 1 << 53, "a Lua number holds it exactly");
        assert_eq!(millis(Duration::from_micros(500)), 1, "never zero");
    }

    /// Every key this crate writes, beside the span target of the crate that
    /// owns its concern — the queue's, the scheduler's, the rate limiter's,
    /// never `redis`.
    fn every_key() -> Vec<(&'static str, &'static str)> {
        let queue = [
            JOBS,
            ENTRIES,
            DUE,
            DELAYED,
            UNIQUE,
            CLAIMS,
            DEFERRED,
            CHECKPOINTS,
            THROTTLE,
            DEAD,
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
    /// or fails here — a queue's naming its queue first, in a hash tag, and no
    /// key another's twin or its prefix but at a level, so a `SCAN` of one never
    /// matches the other.
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
                assert_eq!(levels.len(), 4, "{key} is `nestrs:queue:{{}}:<structure>`");
                assert_eq!(levels[2], HASH_TAG, "{key} names its queue first");
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
    }

    /// A queue's keys share one hash tag, its name, so they share one slot —
    /// and one queue's keys never prefix another's, whose name it prefixes. A
    /// name holding a brace would end the tag inside it, so the port refuses one.
    #[test]
    fn a_queues_keys_share_its_hash_tag_and_no_other_queues() {
        for braced in ["a{b", "a}b", "{audio}"] {
            assert!(QueueName::new(braced).is_err(), "{braced} is refused");
        }
        let audio = QueueKeys::new(&QueueName::new("audio").expect("a valid name"));
        let longer = QueueKeys::new(&QueueName::new("audio2").expect("a valid name"));
        assert_eq!(audio.jobs, "nestrs:queue:{audio}:jobs");
        assert_eq!(audio.dead, "nestrs:queue:{audio}:dead");
        for key in [
            &audio.jobs,
            &audio.entries,
            &audio.due,
            &audio.delayed,
            &audio.unique,
            &audio.claims,
            &audio.deferred,
            &audio.checkpoints,
            &audio.throttle,
            &audio.dead,
        ] {
            assert!(key.starts_with("nestrs:queue:{audio}:"), "{key}");
            assert!(!key.starts_with("nestrs:queue:{audio2}"), "{key}");
        }
        assert!(!longer.jobs.starts_with("nestrs:queue:{audio}"));
    }
}
