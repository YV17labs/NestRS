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

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::SystemTime;

use async_trait::async_trait;
use nest_rs_queue::{
    Envelope, JobId, JobProducer, PushOptions, QueueBackend, QueueError, QueueName,
};

use super::scripts::{SCRIPTS, keyed};
use crate::RedisConnection;
use crate::backend::BACKEND;
use crate::error::UnexpectedReply;
use crate::layout::{GROUP, QueueKeys, millis};

/// The most queues whose keys a producer keeps spelled: a name pushed to past
/// them — the raw push takes any — has its keys spelled at each call instead,
/// so names read from input cannot grow the producer without bound.
const KEYS_KEPT: usize = 1024;

/// The producer a feature pushes through. Bound by
/// [`RedisQueueModule`](crate::RedisQueueModule) under both its own name and
/// `Arc<dyn JobProducer>`; a `Clone` shares the underlying connection.
///
/// It honours every capability the crate's backend declaration names: a delay,
/// a unique key, a cancel by receipt or by unique key.
#[derive(Clone)]
pub struct RedisQueueProducer {
    conn: RedisConnection,
    keys: Arc<KeptKeys>,
}

/// Every key of each queue pushed to, spelled once, for at most [`KEYS_KEPT`]
/// queues.
#[derive(Default)]
struct KeptKeys(RwLock<HashMap<QueueName, Arc<QueueKeys>>>);

impl KeptKeys {
    fn of(&self, queue: &QueueName) -> Arc<QueueKeys> {
        if let Some(keys) = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(queue)
        {
            return Arc::clone(keys);
        }
        let keys = Arc::new(QueueKeys::new(queue));
        let mut kept = self.0.write().unwrap_or_else(PoisonError::into_inner);
        if kept.len() < KEYS_KEPT {
            kept.insert(queue.clone(), Arc::clone(&keys));
        }
        keys
    }
}

impl RedisQueueProducer {
    /// A producer over the app's shared connection (reused, never reopened).
    pub fn new(conn: RedisConnection) -> Self {
        Self {
            conn,
            keys: Arc::default(),
        }
    }

    /// Load the scripts a push and a cancel run, so the first of each is one
    /// round trip rather than a refused call, a load and the call again.
    pub(crate) async fn load_scripts(&self) -> Result<(), redis::RedisError> {
        let mut conn = self.conn.clone();
        for script in SCRIPTS.producer() {
            script.load_async(&mut conn).await?;
        }
        Ok(())
    }

    /// Cancel the job `job` names, or the one holding the unique key `key`.
    async fn cancel(&self, queue: &QueueName, job: &str, key: &str) -> Result<bool, QueueError> {
        let keys = self.keys.of(queue);
        let cancelled: i64 = self
            .conn
            .invoke(
                keyed(&SCRIPTS.cancel, &keys.transition()[..8])
                    .arg(GROUP)
                    .arg(job)
                    .arg(key),
            )
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
        let keys = self.keys.of(queue);
        let mut push = keyed(&SCRIPTS.push, &keys.transition()[..7]);
        push.arg(after);
        let mut unique_keys = Vec::with_capacity(envelopes.len());
        for envelope in envelopes {
            let id = envelope.id().to_string();
            let unique = envelope.unique_key().unwrap_or_default().to_owned();
            let record = serde_json::to_vec(&envelope.into_json())?;
            push.arg(id).arg(record).arg(&unique);
            unique_keys.push(unique);
        }
        let answer: redis::Value = self.conn.invoke(&push).await.map_err(QueueError::backend)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(name: impl Into<String>) -> QueueName {
        QueueName::new(name.into()).expect("a valid name")
    }

    /// A queue's keys are spelled once, and a name past the bound — the raw
    /// push takes any — is spelled per call rather than kept.
    #[test]
    fn a_queues_keys_are_spelled_once_and_no_more_queues_are_kept_than_the_bound() {
        let kept = KeptKeys::default();
        let orders = queue("orders");
        assert!(Arc::ptr_eq(&kept.of(&orders), &kept.of(&orders)));
        for at in 1..KEYS_KEPT {
            kept.of(&queue(format!("queue-{at}")));
        }
        let past = queue("past-the-bound");
        assert!(!Arc::ptr_eq(&kept.of(&past), &kept.of(&past)));
        assert!(Arc::ptr_eq(&kept.of(&orders), &kept.of(&orders)));
    }
}
