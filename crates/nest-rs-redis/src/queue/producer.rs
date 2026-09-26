//! [`RedisQueueProducer`] — the queue port's producer half over the shared
//! [`RedisConnection`]: the [`JobProducer`] binding a feature injects as
//! `Arc<dyn JobProducer>` to push without naming the backend.
//!
//! The port checks the queue and the options, mints each job's id and seals
//! every job before this type sees it; what is stored is that sealed envelope,
//! as the port handed it, pushed onto an apalis `RedisStorage<serde_json::Value>`
//! namespaced by the queue's name — which is how apalis routes a job to the
//! worker draining that queue. Every job is a JSON `Value` at this boundary, so
//! the worker never names the payload type. The receipt a caller gets names the
//! port's id; apalis's own task id for the record stays inside this crate, and
//! reaches a job's lines only as its `backend_id`.

use apalis::prelude::Storage;
use apalis_redis::{Config, RedisStorage};
use async_trait::async_trait;
use nest_rs_queue::{Envelope, JobProducer, PushOptions, QueueBackend, QueueError, QueueName};

use crate::RedisConnection;
use crate::backend::BACKEND;

/// The producer a feature pushes through. Bound by
/// [`RedisQueueModule`](crate::RedisQueueModule) under both its own name and
/// `Arc<dyn JobProducer>`; a `Clone` shares the underlying connection.
///
/// It declares no optional capability (see the crate's backend declaration), so
/// a delayed push, a unique key and a cancel are refused by the port before they
/// reach it, naming this backend.
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
    fn storage(&self, queue: &QueueName) -> RedisStorage<serde_json::Value, RedisConnection> {
        RedisStorage::new_with_config(
            self.conn.clone(),
            Config::default().set_namespace(queue.as_str()),
        )
    }
}

#[async_trait]
impl JobProducer for RedisQueueProducer {
    fn backend(&self) -> &'static QueueBackend {
        &BACKEND
    }

    async fn enqueue(
        &self,
        queue: &QueueName,
        envelopes: Vec<Envelope>,
        _options: &PushOptions,
    ) -> Result<(), QueueError> {
        // `_options` carries nothing this backend could act on: every option
        // needs a capability it does not declare, and the port refused those.
        //
        // `push` takes `&mut self`; storage is a cheap clone of the connection
        // handle, so build one per call rather than force callers to hold it mut.
        let mut storage = self.storage(queue);
        for envelope in envelopes {
            storage
                .push(envelope.into_json())
                .await
                .map_err(QueueError::backend)?;
        }
        Ok(())
    }
}
