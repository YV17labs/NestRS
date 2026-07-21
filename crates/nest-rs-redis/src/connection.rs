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
use nest_rs_queue::{Job, JobProducer, QueueError, WIRE_FORMAT_VERSION};
use serde_json::json;

use crate::error::RedisError;

/// The app's shared Redis connection — queue-flavoured by history, not
/// queue-only. It is seeded once by [`QueueModule`](crate::QueueModule) and
/// injected by producers; other Redis-backed features enabled on this crate
/// (the `throttler` rate-limit store, a future cache/locks) reuse the very same
/// multiplexed handle via [`manager`](Self::manager) instead of opening a
/// second connection.
#[derive(Clone)]
pub struct QueueConnection {
    conn: ConnectionManager,
}

impl QueueConnection {
    /// Open a multiplexed Redis connection to `redis_url`.
    pub async fn connect(redis_url: &str) -> Result<Self, RedisError> {
        let conn = apalis_redis::connect(redis_url).await?;
        Ok(Self { conn })
    }

    /// A cheap clone of the multiplexed connection handle, shared with
    /// non-queue Redis features enabled on this crate (rate-limit store, cache,
    /// distributed locks). `ConnectionManager` is `Clone` and every clone talks
    /// over the one underlying connection, so this is the reuse seam — no second
    /// connect.
    ///
    /// **Do not run blocking commands on it** (`BLPOP`, `BRPOP`, `WAIT`, a
    /// `SUBSCRIBE` that parks the socket): the handle multiplexes every caller
    /// over a single connection, so a blocking command would stall all other
    /// users. Non-blocking, atomic operations (a `Script`, `INCR`, `GET`/`SET`)
    /// are the intended traffic.
    pub fn manager(&self) -> ConnectionManager {
        self.conn.clone()
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
    /// Serialize `job` and enqueue it onto this queue's Redis storage.
    pub async fn push(&self, job: J) -> Result<(), QueueError> {
        let payload = serde_json::to_value(&job)?;
        // `push` takes `&mut self`; storage is a cheap clone of the connection
        // handle, so clone per call rather than force callers to hold it mut.
        let mut storage = self.storage.clone();
        storage
            .push(envelope(payload))
            .await
            .map_err(QueueError::backend)?;
        Ok(())
    }
}

/// Backend-agnostic producer surface — any feature injecting
/// `Arc<dyn JobProducer>` (instead of the concrete `QueueConnection`) is
/// portable across backends.
#[async_trait]
impl JobProducer for QueueConnection {
    async fn push_json(&self, queue: &str, payload: serde_json::Value) -> Result<(), QueueError> {
        let mut storage = self.value_storage(queue);
        storage
            .push(envelope(payload))
            .await
            .map_err(QueueError::backend)?;
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
