//! [`RedisJob`] — the record the queue binding stores and the worker binding
//! reads back, and the one place the key layout both of them share is decided.
//!
//! oxana routes a job to a worker by its *name* and hands that worker only the
//! job's arguments. Routing by queue is the port's, not oxana's, so every queue
//! this crate serves is drained by one worker: the name is one constant, and the
//! queue travels inside the record, because the worker needs it to find the
//! `#[process]` method and the arguments would otherwise not say it.

use serde::{Deserialize, Serialize};

use crate::RedisConnection;
use crate::error::ReservedQueueName;

/// The prefix of every key the queue bindings write — `nestrs:queue:queue:<name>`
/// (a queue's pending list), `nestrs:queue:jobs`, `nestrs:queue:dead`. It sits
/// beside the throttler's `nestrs:throttle:`.
const NAMESPACE: &str = "nestrs:queue";

/// The job name oxana stores and routes on. Never derived from a Rust path: a
/// job waiting in Redis across a deploy is matched to its worker by this string,
/// and a refactor moving a type must not orphan it. Spelled like the keys beside
/// it rather than like a path, which would read as a span target.
const JOB_NAME: &str = "nestrs:queue:job";

/// One queued message, as the producer writes it.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct RedisJob {
    /// The queue it was pushed to — what the worker routes on.
    pub(crate) queue: String,
    /// The port's sealed envelope, opened by `nest_rs_queue::consume::attempt`.
    pub(crate) message: serde_json::Value,
}

impl oxana::Job for RedisJob {
    fn name() -> &'static str {
        JOB_NAME
    }
}

/// A record as the worker receives it: any JSON at all under [`JOB_NAME`].
///
/// Decoding a [`RedisJob`] is the worker's, not oxana's, so a record the
/// producer did not write reaches the worker and fails there, with an event on
/// the framework's own target. Decoded by oxana, it failed inside the runtime
/// with a line no framework filter selects. A record under another job name
/// still fails inside oxana — nothing here is registered to receive it.
#[derive(Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Received(pub(crate) serde_json::Value);

impl oxana::Job for Received {
    fn name() -> &'static str {
        JOB_NAME
    }
}

impl RedisJob {
    /// The storage every `RedisJob` lives in, over the shared pool. A handle
    /// rather than a connection: building one names the keys and opens nothing.
    pub(crate) fn storage(conn: &RedisConnection) -> Result<oxana::Storage, oxana::OxanaError> {
        oxana::Storage::builder()
            .namespace(NAMESPACE)
            .build_from_pool(conn.pool())
    }

    /// Refuse a queue name oxana would take verbatim instead of prefixing — one
    /// starting with [`NAMESPACE`] — because verbatim it lands on the bindings'
    /// own keys.
    pub(crate) fn check_queue(queue: &str) -> Result<(), ReservedQueueName> {
        if queue.starts_with(NAMESPACE) {
            return Err(ReservedQueueName {
                queue: queue.to_owned(),
                namespace: NAMESPACE,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queue_under_the_namespace_is_refused_and_any_other_is_not() {
        for reserved in ["nestrs:queue:dead", "nestrs:queue:retry", "nestrs:queueish"] {
            let error = RedisJob::check_queue(reserved).expect_err(reserved);
            assert!(error.to_string().contains(reserved), "{error}");
        }
        for queue in [
            "audio",
            "nestrs-e2e-probe",
            "nestrs:throttle",
            "queue:nestrs:queue",
        ] {
            assert!(RedisJob::check_queue(queue).is_ok(), "{queue}");
        }
    }
}
