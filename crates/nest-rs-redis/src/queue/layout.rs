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
//! | `…:dead` | stream | one entry per dead letter: `job`, `record`, `reason`, and `unique` when it held a key | the job dead-letters | 7 days or 10,000 entries |
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
    /// The keys a transition's script reads, in the order it reads them as
    /// `KEYS[1]`, `KEYS[2]`, …; each script takes as many from the front as it
    /// needs.
    pub(crate) fn transition(&self) -> [&str; 9] {
        [
            &self.jobs,
            &self.entries,
            &self.due,
            &self.delayed,
            &self.unique,
            &self.claims,
            &self.deferred,
            &self.checkpoints,
            &self.dead,
        ]
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_a_level_of_the_queue_named_first_in_its_hash_tag() {
        let keys = [
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
        crate::testing::assert_keys_of(nest_rs_queue::TARGET, &keys);
        for key in keys {
            let levels: Vec<_> = key.split(':').collect();
            assert_eq!(levels.len(), 4, "{key} is `nestrs:queue:{{}}:<structure>`");
            assert_eq!(levels[2], HASH_TAG, "{key} names its queue first");
        }
    }

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
