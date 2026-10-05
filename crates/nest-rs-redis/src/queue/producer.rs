//! [`RedisQueueProducer`] — the queue port's producer half over the shared
//! [`RedisConnection`]: the [`JobProducer`] binding a feature injects as
//! `Arc<dyn JobProducer>` to push without naming the backend.
//!
//! The port checks the queue and the options, mints each job's id and seals
//! every job before this type sees it; what is stored is that sealed envelope,
//! as the port handed it, keyed by the port's id ([`crate::layout`]).
//!
//! **A push is one script**: it refuses a unique key another job holds before
//! anything is filed, then files every job — on the stream, or held back until
//! Redis's clock says it is due — and claims its key, so a push lands whole or
//! not at all, and a push whose answer never came either filed every job or
//! none. **A cancel is one script too**: it removes a job still waiting and
//! everything it held, and refuses one a delivery runs.

use std::time::SystemTime;

use async_trait::async_trait;
use nest_rs_queue::{
    Envelope, JobId, JobProducer, PushOptions, QueueBackend, QueueError, QueueName,
};

use super::scripts::SCRIPTS;
use crate::RedisConnection;
use crate::backend::BACKEND;
use crate::error::UnexpectedReply;
use crate::layout::{GROUP, QueueKeys, millis};

/// The producer a feature pushes through. Bound by
/// [`RedisQueueModule`](crate::RedisQueueModule) under both its own name and
/// `Arc<dyn JobProducer>`; a `Clone` shares the underlying connection.
///
/// It honours every capability the crate's backend declaration names: a delay,
/// a unique key, a cancel by receipt or by unique key.
#[derive(Clone)]
pub struct RedisQueueProducer {
    conn: RedisConnection,
}

impl RedisQueueProducer {
    /// A producer over the app's shared connection (reused, never reopened).
    pub fn new(conn: RedisConnection) -> Self {
        Self { conn }
    }

    /// Cancel the job `job` names, or the one holding the unique key `key`.
    async fn cancel(&self, queue: &QueueName, job: &str, key: &str) -> Result<bool, QueueError> {
        let keys = QueueKeys::new(queue);
        let cancelled: i64 = SCRIPTS
            .cancel
            .key(&keys.jobs)
            .key(&keys.entries)
            .key(&keys.due)
            .key(&keys.delayed)
            .key(&keys.unique)
            .key(&keys.claims)
            .key(&keys.deferred)
            .key(&keys.checkpoints)
            .arg(GROUP)
            .arg(job)
            .arg(key)
            .invoke_async(&mut self.conn.clone())
            .await
            .map_err(QueueError::backend)?;
        Ok(cancelled == 1)
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
        options: &PushOptions,
    ) -> Result<(), QueueError> {
        let now = SystemTime::now();
        // The delay is measured here and its instant taken on Redis's clock, so
        // a host whose clock drifts holds a job back for the delay it asked.
        let after = match options.delay() {
            None => 0,
            // An instant already past is an immediate push.
            Some(delay) => delay
                .deadline(now)?
                .duration_since(now)
                .ok()
                .filter(|wait| !wait.is_zero())
                .map_or(0, millis),
        };
        let keys = QueueKeys::new(queue);
        let mut push = SCRIPTS.push.key(&keys.jobs);
        push.key(&keys.entries)
            .key(&keys.due)
            .key(&keys.delayed)
            .key(&keys.unique)
            .key(&keys.claims)
            .key(&keys.deferred)
            .arg(after);
        let mut unique_keys = Vec::with_capacity(envelopes.len());
        for envelope in envelopes {
            let id = envelope.id().to_string();
            let unique = envelope.unique_key().unwrap_or_default().to_owned();
            let record = serde_json::to_vec(&envelope.into_json())?;
            push.arg(id).arg(record).arg(&unique);
            unique_keys.push(unique);
        }
        let answer: redis::Value = push
            .invoke_async(&mut self.conn.clone())
            .await
            .map_err(QueueError::backend)?;
        match answer {
            redis::Value::Int(0) => Ok(()),
            redis::Value::Array(refused) => {
                let held = redis::from_redis_value::<(usize, String)>(redis::Value::Array(refused))
                    .ok()
                    .and_then(|(at, holder)| Some((unique_keys.get(at)?.clone(), holder)));
                let Some((key, holder)) = held else {
                    return Err(QueueError::backend(UnexpectedReply {
                        call: "the push",
                        expected: "the position of a job pushed, and a job id",
                    }));
                };
                Err(QueueError::UniqueKeyHeld {
                    queue: queue.clone(),
                    key,
                    holder: JobId::parse(&holder)?,
                })
            }
            _ => Err(QueueError::backend(UnexpectedReply {
                call: "the push",
                expected: "0, or a position and a job id",
            })),
        }
    }

    async fn remove(&self, queue: &QueueName, id: &JobId) -> Result<bool, QueueError> {
        self.cancel(queue, &id.to_string(), "").await
    }

    async fn remove_unique(&self, queue: &QueueName, key: &str) -> Result<bool, QueueError> {
        self.cancel(queue, "", key).await
    }
}
