//! Shared Redis connection + typed [`Queue`] producer handle. The queue name
//! supplied at the call site must match the consuming `#[processor(queue = ...)]`.
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

use std::marker::PhantomData;

use apalis::prelude::Storage;
use apalis_redis::{Config, ConnectionManager, RedisStorage};
use async_trait::async_trait;
use nest_rs_queue::{Job, JobProducer, WIRE_FORMAT_VERSION};
use serde_json::json;

#[derive(Clone)]
pub struct QueueConnection {
    conn: ConnectionManager,
}

impl QueueConnection {
    pub async fn connect(redis_url: &str) -> anyhow::Result<Self> {
        let conn = apalis_redis::connect(redis_url).await?;
        Ok(Self { conn })
    }

    /// Typed producer handle. `J` is the job type the consumer expects; the
    /// payload is serialized to JSON on the wire (matches the consumer's
    /// `JobHandler` deserializing from `serde_json::Value`).
    pub fn of<J: Job>(&self, queue: &str) -> Queue<J> {
        Queue {
            storage: self.value_storage(queue),
            _phantom: PhantomData,
        }
    }

    /// Producer-side storage handle. Configured to namespace under `queue`
    /// just like the consumer — this is how apalis routes a job to the right
    /// worker.
    pub(crate) fn value_storage(&self, queue: &str) -> RedisStorage<serde_json::Value> {
        RedisStorage::new_with_config(self.conn.clone(), Config::default().set_namespace(queue))
    }

    /// Consumer-side storage. Fetch buffer = processor concurrency — the
    /// in-flight-job ceiling.
    pub(crate) fn consumer_storage(
        &self,
        queue: &str,
        concurrency: usize,
    ) -> RedisStorage<serde_json::Value> {
        RedisStorage::new_with_config(
            self.conn.clone(),
            Config::default()
                .set_namespace(queue)
                .set_buffer_size(concurrency.max(1)),
        )
    }
}

/// Typed producer handle returned by [`QueueConnection::of`]. The `J` is a
/// compile-time aid for the call site — the wire payload is always JSON.
pub struct Queue<J: Job> {
    storage: RedisStorage<serde_json::Value>,
    _phantom: PhantomData<fn(J)>,
}

impl<J: Job> Queue<J> {
    pub async fn push(&self, job: J) -> anyhow::Result<()> {
        let payload = serde_json::to_value(&job)?;
        // `push` takes `&mut self`; storage is a cheap clone of the connection
        // handle, so clone per call rather than force callers to hold it mut.
        let mut storage = self.storage.clone();
        storage.push(envelope(payload)).await?;
        Ok(())
    }
}

/// Backend-agnostic producer surface — any feature injecting
/// `Arc<dyn JobProducer>` (instead of the concrete `QueueConnection`) is
/// portable across backends.
#[async_trait]
impl JobProducer for QueueConnection {
    async fn push_json(&self, queue: &str, payload: serde_json::Value) -> anyhow::Result<()> {
        let mut storage = self.value_storage(queue);
        storage.push(envelope(payload)).await?;
        Ok(())
    }
}

/// Wrap a user payload in the wire envelope the consumer expects. Bumping
/// [`WIRE_FORMAT_VERSION`] lets a rolling deploy fail closed instead of
/// misinterpreting bytes.
fn envelope(payload: serde_json::Value) -> serde_json::Value {
    json!({
        "v": WIRE_FORMAT_VERSION,
        "payload": payload,
    })
}
