//! [`RedisQueueProducer`] — the queue port's producer half over the shared
//! [`RedisConnection`]: the [`JobProducer`] binding a feature injects as
//! `Arc<dyn JobProducer>` to enqueue without naming the backend.
//!
//! Wire format is a JSON **envelope** — `{ "v": <number>, "payload": <user
//! payload> }` — carried in a [`RedisJob`] that names its queue. The matching
//! consumer (the `#[processor]` macro-emitted `JobHandler`) unwraps the
//! envelope, switches on `v`, and deserializes `payload` to the user's job
//! type. Unversioned legacy values are decoded directly with a warning, so a
//! payload written before envelopes existed still runs — provided it sits in
//! this backend's key layout, which a job from a release on another backend
//! does not. This is the seam that lets the `#[processor]` macro stay
//! backend-agnostic: any backend can drain the `ProcessMethod` inventory because
//! every job is a JSON `Value` at the boundary.

use async_trait::async_trait;
use nest_rs_queue::{JobProducer, QueueError};
use oxana::{Queue, QueueConfig};

use crate::RedisConnection;
use crate::job::RedisJob;

/// The producer a feature pushes through. Bound by
/// [`RedisQueueModule`](crate::RedisQueueModule) under both its own name and
/// `Arc<dyn JobProducer>`; a `Clone` shares the underlying pool.
#[derive(Clone)]
pub struct RedisQueueProducer {
    conn: RedisConnection,
}

impl RedisQueueProducer {
    /// A producer over the app's shared pool (reused, never reopened).
    pub fn new(conn: RedisConnection) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl JobProducer for RedisQueueProducer {
    async fn push_json(&self, queue: &str, payload: serde_json::Value) -> Result<(), QueueError> {
        // The raw hatch takes any string, and a reserved one would write onto
        // the bindings' own keys and report success.
        RedisJob::check_queue(queue).map_err(QueueError::backend)?;
        // A storage handle per call: it names the keys and opens nothing, so
        // there is no connection to hold between pushes.
        let storage = RedisJob::storage(&self.conn).map_err(QueueError::backend)?;
        let job = RedisJob {
            queue: queue.to_owned(),
            message: nest_rs_queue::envelope::seal(payload),
        };
        storage
            .enqueue(QueueKey(queue), job)
            .await
            .map_err(QueueError::backend)?;
        Ok(())
    }
}

/// A queue named at run time, which is what `push_json` receives.
///
/// That is oxana's *dynamic* shape: the type states the kind, the instance
/// supplies the key. Enqueueing reads the key alone — which queues are drained,
/// and at what concurrency, is the worker's to declare.
struct QueueKey<'a>(&'a str);

impl Queue for QueueKey<'_> {
    fn key(&self) -> String {
        self.0.to_owned()
    }

    fn to_config() -> QueueConfig {
        QueueConfig::as_dynamic("")
    }
}
