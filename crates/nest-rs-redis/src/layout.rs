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
//! own records for a queue (a delivery's lease, a job's settled mark) are levels
//! of the same namespace, under words apalis does not use.
//!
//! **6.x handed apalis the queue's bare name**, so its jobs sit at the root —
//! `<queue>:active`, `<queue>:scheduled`, and the one in-flight set its worker
//! kept, named for the queue as well. A 7.0 worker reads none of them, so jobs
//! left there would wait forever without a word; [`legacy_jobs`] finds them, the
//! worker refuses to start beside them, and the producer says so once per queue.

use apalis_redis::Config;
use nest_rs_queue::QueueName;

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
pub(crate) const QUEUE_SLOT: &str = "{queue}";

/// The namespace `queue`'s records live under — apalis's structures and the
/// framework's own.
pub(crate) fn namespace(queue: &QueueName) -> String {
    NAMESPACE.replace(QUEUE_SLOT, queue.as_str())
}

/// The storage settings every handle on `queue` starts from: its namespace. A
/// producer uses them as they are; the worker adds its fetch and liveness
/// settings on top.
pub(crate) fn config(queue: &QueueName) -> Config {
    Config::default().set_namespace(&namespace(queue))
}

/// The keys under which 6.x kept jobs that have not finished — waiting, held
/// for later, or taken by a worker — for `queue`. Its namespace was the queue's
/// bare name, and so was its worker's id, which is why the in-flight set is
/// named for the queue twice. Exact names rather than a `SCAN`: an ACL scoped to
/// this deployment's keys can answer three names, and a pattern over the whole
/// keyspace cannot be answered cheaply by anyone.
fn legacy_keys(queue: &QueueName) -> [String; 3] {
    let queue = queue.as_str();
    [
        format!("{queue}:active"),
        format!("{queue}:scheduled"),
        format!("{queue}:inflight:{queue}"),
    ]
}

/// The 6.x keys still holding jobs for `queue` — empty when there are none.
/// Read only: one `EXISTS` per name, in one round trip. Redis deletes an empty
/// list, set or sorted set, so a name that exists holds at least one job.
pub(crate) async fn legacy_jobs(
    conn: &RedisConnection,
    queue: &QueueName,
) -> Result<Vec<String>, redis::RedisError> {
    let keys = legacy_keys(queue);
    let mut pipeline = redis::pipe();
    for key in &keys {
        pipeline.exists(key);
    }
    let present: Vec<bool> = pipeline.query_async(&mut conn.clone()).await?;
    Ok(keys
        .into_iter()
        .zip(present)
        .filter_map(|(key, present)| present.then_some(key))
        .collect())
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

    /// Every 6.x name is the queue's own at the root, which the framework's
    /// namespace never is — so a check can never mistake one for the other.
    #[test]
    fn the_legacy_names_sit_at_the_root_under_the_queues_bare_name() {
        let root = format!("{}:", audio());
        for key in legacy_keys(&audio()) {
            assert!(key.starts_with(&root), "{key}");
            assert!(!key.starts_with(&namespace(&audio())), "{key}");
        }
    }
}
