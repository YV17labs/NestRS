//! [`LegacyLayout`] — what the 6.x key layout still holds for a queue, read so
//! a 7.0 worker never starts beside jobs it would leave waiting.
//!
//! **6.x handed apalis the queue's bare name**, so its jobs sit at the root of
//! the keyspace — waiting on `<queue>:active`, held back for later on
//! `<queue>:scheduled`, and in flight in the sets its workers registered in
//! `<queue>:consumers`. A 7.0 worker reads none of them, so jobs left there
//! would wait forever without a word: the worker refuses to start while one
//! holds a job, and the producer warns once per queue.
//!
//! **This file is the one exception to "apalis's structures are apalis's".** The
//! framework reaches apalis's keys only through apalis's public API, and here
//! alone it reads them itself, because no public call answers exactly: apalis's
//! calls count no schedule, and its `stats` script reads five names in one
//! script that a key of another type at any one of them fails whole — an
//! application's counter at `<queue>:failed` hid every 6.x job. The read is
//! bounded on every side, and the conformance keys join holds it here:
//!
//! - **the names are apalis's**, read off its `Config` getters under the 6.x
//!   namespace — and the in-flight sets are the members apalis registered in
//!   the consumers set under its in-flight prefix — never spelled here, and no
//!   other name is read;
//! - **the commands only read**: `TYPE`, then `LLEN`, `ZCARD`, `ZRANGE` or
//!   `SCARD` for the type found — never a write, never a script;
//! - **a key of another type is not 6.x's**: left alone, not counted, and named
//!   in one `warn`, since a structure the check cannot read is one it cannot
//!   vouch for.
//!
//! Never a `SCAN`, which costs the whole keyspace and which an ACL confined to
//! `nestrs:*` refuses; a `NOPERM` answer is the caller's to say
//! ([`outside_the_acl`]), never taken for an empty layout.

use apalis_redis::Config;
use nest_rs_queue::QueueName;

use crate::RedisConnection;

/// How many members of the consumers set one `ZRANGE` reads. 6.x gave every
/// worker of a queue the queue's name as its id, so its set holds one member;
/// the read is chunked anyway, so a set however large never costs Redis one
/// command of unbounded size.
const CONSUMERS_PER_READ: isize = 1_000;

/// What the 6.x layout holds for one queue: the structures holding jobs, and
/// the keys at a 6.x name holding something else.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LegacyLayout {
    held: Vec<Held>,
    foreign: Vec<Foreign>,
}

/// One 6.x structure holding jobs.
#[derive(Debug, PartialEq, Eq)]
struct Held {
    key: String,
    jobs: u64,
    state: JobState,
}

/// Where a job waits in the 6.x layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobState {
    /// On the list apalis fetches from.
    Waiting,
    /// On the schedule — a delayed push, a retry's next attempt.
    Scheduled,
    /// Fetched by a 6.x worker and not settled.
    InFlight,
}

/// A key at a 6.x name that is not the structure 6.x kept there.
#[derive(Debug, PartialEq, Eq)]
struct Foreign {
    key: String,
    /// What the key holds, in Redis's word — or why a sorted set at the
    /// consumers name is not apalis's.
    holds: String,
}

/// The type a 6.x structure has, in Redis's `TYPE` word.
const LIST: &str = "list";
const ZSET: &str = "zset";
const SET: &str = "set";
/// What `TYPE` answers for a key that is not there.
const ABSENT: &str = "none";

impl LegacyLayout {
    /// Read what the 6.x layout holds for `queue`, through `conn` and within
    /// its budget, saying at `warn` which keys at a 6.x name hold something
    /// else.
    pub(crate) async fn read(
        conn: &RedisConnection,
        queue: &QueueName,
    ) -> Result<Self, redis::RedisError> {
        let layout = legacy_config(queue);
        let active = layout.active_jobs_list();
        let scheduled = layout.scheduled_jobs_set();
        let consumers = layout.consumers_set();
        let mut found = Self::default();

        let (active_type, scheduled_type, consumers_type) = tokio::try_join!(
            key_type(conn, &active),
            key_type(conn, &scheduled),
            key_type(conn, &consumers),
        )?;
        if found.expect(&active, &active_type, LIST) {
            let jobs: u64 = redis::cmd("LLEN")
                .arg(&active)
                .query_async(&mut conn.clone())
                .await?;
            found.count(active, jobs, JobState::Waiting);
        }
        if found.expect(&scheduled, &scheduled_type, ZSET) {
            let jobs: u64 = redis::cmd("ZCARD")
                .arg(&scheduled)
                .query_async(&mut conn.clone())
                .await?;
            found.count(scheduled, jobs, JobState::Scheduled);
        }
        if found.expect(&consumers, &consumers_type, ZSET) {
            let prefix = format!("{}:", layout.inflight_jobs_set());
            found.read_in_flight(conn, &consumers, &prefix).await?;
        }
        found.say_foreign(queue);
        Ok(found)
    }

    /// Whether 6.x left no job for the queue.
    pub(crate) fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// Each structure holding jobs and how many, as a sentence names them —
    /// `3 waiting on audio:active`, `1 in flight in audio:inflight:audio`.
    pub(crate) fn held(&self) -> Vec<String> {
        self.held
            .iter()
            .map(|held| {
                let state = match held.state {
                    JobState::Waiting => "waiting on",
                    JobState::Scheduled => "held for later on",
                    JobState::InFlight => "in flight in",
                };
                format!("{} {state} {}", held.jobs, held.key)
            })
            .collect()
    }

    /// The in-flight sets the consumers set names, read a chunk at a time:
    /// each member under apalis's in-flight prefix is a set a 6.x worker filled,
    /// and one that is not means the sorted set is not apalis's, so none of its
    /// members is read.
    async fn read_in_flight(
        &mut self,
        conn: &RedisConnection,
        consumers: &str,
        prefix: &str,
    ) -> Result<(), redis::RedisError> {
        let mut start: isize = 0;
        loop {
            let members: Vec<String> = redis::cmd("ZRANGE")
                .arg(consumers)
                .arg(start)
                .arg(start + CONSUMERS_PER_READ - 1)
                .query_async(&mut conn.clone())
                .await?;
            if members.iter().any(|member| !member.starts_with(prefix)) {
                self.foreign.push(Foreign {
                    key: consumers.to_owned(),
                    holds: "a zset of names apalis never registers".to_owned(),
                });
                self.held.retain(|held| held.state != JobState::InFlight);
                return Ok(());
            }
            for set in &members {
                let found = key_type(conn, set).await?;
                if self.expect(set, &found, SET) {
                    let jobs: u64 = redis::cmd("SCARD")
                        .arg(set)
                        .query_async(&mut conn.clone())
                        .await?;
                    self.count(set.clone(), jobs, JobState::InFlight);
                }
            }
            if members.len() < CONSUMERS_PER_READ.unsigned_abs() {
                return Ok(());
            }
            start += CONSUMERS_PER_READ;
        }
    }

    /// Whether `key`, of type `found`, is the structure 6.x kept there — of
    /// `expected` type — noting it when it holds something else. An absent key
    /// is 6.x's, empty.
    fn expect(&mut self, key: &str, found: &str, expected: &str) -> bool {
        match found {
            ABSENT => false,
            found if found == expected => true,
            other => {
                self.foreign.push(Foreign {
                    key: key.to_owned(),
                    holds: other.to_owned(),
                });
                false
            }
        }
    }

    fn count(&mut self, key: String, jobs: u64, state: JobState) {
        if jobs > 0 {
            self.held.push(Held { key, jobs, state });
        }
    }

    /// Say, once for the queue, which keys at a 6.x name hold something else.
    fn say_foreign(&self, queue: &QueueName) {
        if self.foreign.is_empty() {
            return;
        }
        let keys: Vec<String> = self
            .foreign
            .iter()
            .map(|foreign| format!("{} ({})", foreign.key, foreign.holds))
            .collect();
        tracing::warn!(
            target: nest_rs_queue::TARGET,
            queue = %queue,
            keys = %keys.join(", "),
            "a key at a 6.x queue name holds what 6.x never kept there; left alone, and not \
             counted as jobs",
        );
    }
}

/// The storage settings 6.x handed apalis for `queue`: the queue's bare name as
/// its namespace, from which apalis derives every structure — so a name is
/// read off apalis's getters, never spelled here.
fn legacy_config(queue: &QueueName) -> Config {
    Config::default().set_namespace(queue.as_str())
}

/// The type of `key`, in Redis's word — `none` when it is not there.
async fn key_type(conn: &RedisConnection, key: &str) -> Result<String, redis::RedisError> {
    redis::cmd("TYPE")
        .arg(key)
        .query_async(&mut conn.clone())
        .await
}

/// Whether Redis refused a read of the 6.x keys because the connection's ACL
/// does not reach them — a user scoped to the framework's prefix, which could
/// never have written the 6.x layout either.
pub(crate) fn outside_the_acl(error: &redis::RedisError) -> bool {
    error.code() == Some("NOPERM")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio() -> QueueName {
        QueueName::new("audio").expect("a valid name")
    }

    /// Every 6.x name is apalis's derivation from the queue's bare name, at the
    /// root, which the framework's namespace never is — so a check can never
    /// mistake one for the other.
    #[test]
    fn the_legacy_names_sit_at_the_root_under_the_queues_bare_name() {
        let legacy = legacy_config(&audio());
        let root = format!("{}:", audio());
        let namespace = crate::layout::namespace(&audio());
        for key in [
            legacy.active_jobs_list(),
            legacy.consumers_set(),
            legacy.scheduled_jobs_set(),
            legacy.inflight_jobs_set(),
        ] {
            assert!(key.starts_with(&root), "{key}");
            assert!(!key.starts_with(&namespace), "{key}");
        }
    }

    /// A structure of the type 6.x kept is read; an absent one is 6.x's and
    /// empty; one holding another type is noted, never read — and a structure
    /// holding no job is not named.
    #[test]
    fn a_key_is_read_only_when_it_is_the_structure_6x_kept_there() {
        let legacy = legacy_config(&audio());
        let mut found = LegacyLayout::default();
        assert!(found.expect(&legacy.active_jobs_list(), LIST, LIST));
        assert!(!found.expect(&legacy.scheduled_jobs_set(), ABSENT, ZSET));
        assert!(!found.expect(&legacy.consumers_set(), "string", ZSET));
        assert_eq!(
            found.foreign,
            [Foreign {
                key: legacy.consumers_set(),
                holds: "string".to_owned(),
            }],
        );
        found.count(legacy.active_jobs_list(), 0, JobState::Waiting);
        assert!(found.is_empty(), "a structure holding no job is not named");
    }

    /// Every structure holding jobs is named with how many and where they wait
    /// — the schedule included.
    #[test]
    fn what_6x_left_is_named_by_structure_and_count() {
        let legacy = legacy_config(&audio());
        let in_flight = format!("{}:{}", legacy.inflight_jobs_set(), audio());
        let mut found = LegacyLayout::default();
        found.count(legacy.active_jobs_list(), 3, JobState::Waiting);
        found.count(legacy.scheduled_jobs_set(), 2, JobState::Scheduled);
        found.count(in_flight.clone(), 1, JobState::InFlight);
        assert!(!found.is_empty());
        assert_eq!(
            found.held(),
            [
                format!("3 waiting on {}", legacy.active_jobs_list()),
                format!("2 held for later on {}", legacy.scheduled_jobs_set()),
                format!("1 in flight in {in_flight}"),
            ],
        );
    }

    /// Keys at a 6.x name holding something else are said once, at `warn`,
    /// each with what it holds — and nothing is said when there are none.
    #[test]
    fn keys_holding_something_else_are_said_once_naming_each() {
        let logs = nest_rs_testing::LogCapture::install();
        let legacy = legacy_config(&audio());
        LegacyLayout::default().say_foreign(&audio());
        let mut found = LegacyLayout::default();
        found.expect(&legacy.active_jobs_list(), "string", LIST);
        found.expect(&legacy.scheduled_jobs_set(), "hash", ZSET);
        found.say_foreign(&audio());
        let said = logs.expect_one(
            nest_rs_queue::TARGET,
            "a key at a 6.x queue name holds what 6.x never kept there; left alone, and not \
             counted as jobs",
        );
        assert_eq!(said.level, "warn");
        assert_eq!(
            said.field("keys"),
            Some(format!(
                "{} (string), {} (hash)",
                legacy.active_jobs_list(),
                legacy.scheduled_jobs_set(),
            )),
        );
        assert_eq!(said.field("queue").as_deref(), Some("audio"));
    }
}
