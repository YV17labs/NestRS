//! [`RedisQueueProducer`] — the queue port's producer half over the shared
//! [`RedisConnection`]: the [`JobProducer`] binding a feature injects as
//! `Arc<dyn JobProducer>` to enqueue without naming the backend.
//!
//! Wire format is a JSON **envelope** — `{ "v": <number>, "payload": <user
//! payload> }` — pushed onto an apalis `RedisStorage<serde_json::Value>`. The
//! matching consumer (the `#[processor]` macro-emitted `JobHandler`) unwraps
//! the envelope, switches on `v`, and deserializes `payload` to the user's
//! job type. Unversioned legacy values are decoded directly with a warning so
//! a rolling deploy doesn't drop jobs left in Redis from the prior release.
//! This is the seam that lets the `#[processor]` macro stay backend-agnostic:
//! any backend can drain the `ProcessMethod` inventory because every job is a
//! JSON `Value` at the boundary.

use apalis::prelude::Storage;
use apalis_redis::{Config, RedisStorage};
use async_trait::async_trait;
use nest_rs_queue::{JobProducer, QueueError};

use crate::RedisConnection;

/// The producer a feature pushes through. Bound by
/// [`RedisQueueModule`](crate::RedisQueueModule) under both its own name and
/// `Arc<dyn JobProducer>`; a `Clone` shares the underlying connection.
#[derive(Clone)]
pub struct RedisQueueProducer {
    conn: RedisConnection,
}

impl RedisQueueProducer {
    /// A producer over the app's shared connection (reused, never reopened).
    pub fn new(conn: RedisConnection) -> Self {
        Self { conn }
    }

    /// Producer-side storage handle, namespaced under `queue` just like the
    /// consumer's — this is how apalis routes a job to the right worker.
    fn storage(&self, queue: &str) -> RedisStorage<serde_json::Value> {
        RedisStorage::new_with_config(self.conn.manager(), Config::default().set_namespace(queue))
    }
}

#[async_trait]
impl JobProducer for RedisQueueProducer {
    async fn push_json(&self, queue: &str, payload: serde_json::Value) -> Result<(), QueueError> {
        // `push` takes `&mut self`; storage is a cheap clone of the connection
        // handle, so build one per call rather than force callers to hold it mut.
        let mut storage = self.storage(queue);
        storage
            .push(nest_rs_queue::envelope::seal(payload))
            .await
            .map_err(QueueError::backend)?;
        Ok(())
    }
}
