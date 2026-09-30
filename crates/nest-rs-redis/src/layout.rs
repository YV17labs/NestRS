//! Where a queue lives in Redis: the namespace every queue's records sit under,
//! shared by the producer that files them and the worker that runs them — and
//! the 6.x layout it replaced, which neither side may ignore.
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
//! | `…:attempts:<job>` | the attempts started and not handed back unrun | its first attempt | its terminal outcome |
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
//!
//! **6.x handed apalis the queue's bare name**, so its jobs sit at the root —
//! `<queue>:active`, `<queue>:scheduled`, and the in-flight sets its workers
//! kept. A 7.0 worker reads none of them, so jobs left there would wait forever
//! without a word; [`legacy_jobs`] finds them, the worker refuses to start beside
//! them, and the producer says so once per queue. It asks apalis — its `stats`,
//! on a storage opened under the 6.x namespace — and never reads those keys
//! itself: apalis's structures are apalis's, 6.x's included, and a key of
//! another type at one of those names, an application's own, is apalis's to
//! refuse rather than the framework's to count as jobs.

use std::time::Duration;

use apalis::prelude::BackendExpose;
use apalis_redis::{Config, RedisStorage};
use nest_rs_queue::{JobId, QueueName};

use crate::RedisConnection;

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

/// How many attempts at a job have started and not been handed back unrun —
/// the count the port reads an attempt that never returned from, since the
/// envelope only counts the ones that answered.
pub(crate) const ATTEMPTS: &str = "nestrs:queue:{queue}:attempts:{job}";

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
/// [`SETTLED`], [`CANCELLED`], [`CHECKPOINTS`] or [`ATTEMPTS`].
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

/// `duration` in whole milliseconds, at least one — Redis refuses a zero `PX`.
pub(crate) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis())
        .unwrap_or(u64::MAX)
        .max(1)
}

/// The storage settings every handle on `queue` starts from: its namespace. A
/// producer uses them as they are; the worker adds its fetch and liveness
/// settings on top.
pub(crate) fn config(queue: &QueueName) -> Config {
    Config::default().set_namespace(&namespace(queue))
}

/// The storage settings 6.x handed apalis for `queue`: the queue's bare name as
/// its namespace, from which apalis derives every structure — so a name is read
/// off apalis's getters, never spelled here.
fn legacy_config(queue: &QueueName) -> Config {
    Config::default().set_namespace(queue.as_str())
}

/// The jobs the 6.x layout still holds for `queue`, as apalis counts them — each
/// structure's key and how many — empty when there are none.
///
/// apalis's `stats` on a storage opened under the 6.x namespace, in one script:
/// the jobs waiting on its list, and those in flight in the sets of the workers
/// registered as its consumers. A key of another type at one of those names is
/// not 6.x's — an application's own under the same name — and apalis refuses
/// the read (`WRONGTYPE`); that is said, and nothing is counted, rather than
/// taking it for jobs and printing a move into apalis's list.
///
/// **Not counted: jobs 6.x held back for later.** apalis's public calls count no
/// schedule, and the framework reads apalis's structures through nothing else;
/// the upgrading page has the operator look at it before a 7.0 worker starts.
pub(crate) async fn legacy_jobs(
    conn: &RedisConnection,
    queue: &QueueName,
) -> Result<Vec<String>, redis::RedisError> {
    let layout = legacy_config(queue);
    let storage: RedisStorage<serde_json::Value> =
        RedisStorage::new_with_config(conn.manager(), layout.clone());
    let stats = match conn.bound(storage.stats()).await? {
        Ok(stats) => stats,
        Err(error) if not_the_6x_layout(&error) => {
            tracing::info!(
                target: nest_rs_queue::TARGET,
                queue = %queue,
                error = %nest_rs_core::error_message(&error),
                "a key at a 6.x queue name is not the structure 6.x kept there; left alone, and \
                 not counted as jobs",
            );
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
    };
    let mut held = Vec::new();
    if stats.pending > 0 {
        held.push(format!(
            "{} waiting on {}",
            stats.pending,
            layout.active_jobs_list()
        ));
    }
    if stats.running > 0 {
        held.push(format!(
            "{} in flight in the sets {} lists",
            stats.running,
            layout.consumers_set()
        ));
    }
    Ok(held)
}

/// Whether apalis refused to count the 6.x layout because a key at one of its
/// names holds another type — the answer Redis gives a list or set command on
/// a key that is not one, which a script's call carries in its text on Redis
/// before 7.
fn not_the_6x_layout(error: &redis::RedisError) -> bool {
    error.code() == Some("WRONGTYPE") || error.to_string().contains("WRONGTYPE")
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

    /// The concern is read off the port's span target rather than chosen, so
    /// renaming the target moves the namespace — or fails here.
    #[test]
    fn a_queue_lives_under_the_concern_its_port_emits_on() {
        let concern = nest_rs_queue::TARGET
            .strip_prefix("nest_rs::")
            .expect("a framework target");
        assert_eq!(
            namespace(&audio()).split(':').collect::<Vec<_>>(),
            ["nestrs", concern, "audio"],
        );
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

    /// Every 6.x name is apalis's derivation from the queue's bare name, at the
    /// root, which the framework's namespace never is — so a check can never
    /// mistake one for the other.
    #[test]
    fn the_legacy_names_sit_at_the_root_under_the_queues_bare_name() {
        let legacy = legacy_config(&audio());
        let root = format!("{}:", audio());
        for key in [
            legacy.active_jobs_list(),
            legacy.consumers_set(),
            legacy.scheduled_jobs_set(),
        ] {
            assert!(key.starts_with(&root), "{key}");
            assert!(!key.starts_with(&namespace(&audio())), "{key}");
        }
    }

    /// A key of another type at a 6.x name is refused by Redis, and read as
    /// not 6.x's — whether the code comes back as Redis 7 sends it or inside a
    /// script error's text as Redis 6.2 words it — and nothing else is.
    #[test]
    fn a_key_of_another_type_at_a_6x_name_is_read_as_not_the_6x_layout() {
        let seven = redis::RedisError::from((
            redis::ErrorKind::ExtensionError,
            "WRONGTYPE",
            "Operation against a key holding the wrong kind of value".to_owned(),
        ));
        let six = redis::RedisError::from((
            redis::ErrorKind::ResponseError,
            "An error was signalled by the server",
            "Error running script: @user_script:30: WRONGTYPE Operation against a key holding \
             the wrong kind of value"
                .to_owned(),
        ));
        let refused = redis::RedisError::from(std::io::Error::other("connection reset"));
        assert!(not_the_6x_layout(&seven));
        assert!(not_the_6x_layout(&six));
        assert!(!not_the_6x_layout(&refused));
    }
}
