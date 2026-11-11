//! [`RedisQueueProducer`] — the queue port's producer half over the shared
//! [`RedisConnection`]: the [`JobProducer`] the port's `Jobs` hands each push
//! to, so a feature pushes without naming the backend.
//!
//! What is stored is the sealed envelope as the port handed it, keyed by the
//! port's id ([`super::layout`]).
//!
//! **A push is one script**: it refuses a unique key another job holds before
//! anything is filed, then files every job — on the stream, or held back until
//! Redis's clock says it is due — and claims its key, so a push lands whole or
//! not at all, and a push whose answer never came either filed every job or
//! none. It skips a job its queue still holds, so a push sent again — by the
//! client after a lost answer — files nothing twice while that job lasts; once it
//! ended, the same id is filed as a new job ([`JobProducer::enqueue`]), which
//! at-least-once allows. **A cancel is one script
//! too**: it removes a job still waiting and everything it held, and refuses
//! one a delivery runs.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::SystemTime;

use async_trait::async_trait;
use nest_rs_core::{Instance, Unnamed};
use nest_rs_queue::{
    Delay, Envelope, JobId, JobProducer, PushOptions, QueueBackend, QueueError, QueueName,
};

use super::backend::BACKEND;
use super::error::UnexpectedReply;
use super::layout::{GROUP, QueueKeys};
use super::scripts::SCRIPTS;
use crate::RedisConnection;
use crate::instance::{instance_phrase, typed_import};
use crate::millis::{LONGEST, millis};

/// The most queues whose keys a producer keeps spelled: a name pushed to past
/// them — the raw push takes any — has its keys spelled at each call instead,
/// so names read from input cannot grow the producer without bound.
const KEYS_KEPT: usize = 1024;

const _: () = assert!(
    LONGEST.as_millis() >= Delay::LATEST_DUE.as_millis(),
    "a delay to the latest instant a job may be due at reaches Redis whole",
);

/// The producer a feature's pushes reach. Bound by
/// [`RedisQueueModule`](crate::RedisQueueModule) under its own name and as the
/// port's `Jobs`; a `Clone` shares the underlying connection.
///
/// It honours every capability the crate's backend declaration names: a delay,
/// a unique key, a cancel by receipt or by unique key. `I` is the instance it
/// pushes to.
pub struct RedisQueueProducer<I = Unnamed> {
    conn: RedisConnection<I>,
    keys: Arc<KeptKeys>,
}

impl<I> Clone for RedisQueueProducer<I> {
    fn clone(&self) -> Self {
        Self {
            conn: self.conn.clone(),
            keys: Arc::clone(&self.keys),
        }
    }
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

impl<I: Instance> RedisQueueProducer<I> {
    /// A producer over the app's shared connection (reused, never reopened).
    pub fn new(conn: RedisConnection<I>) -> Self {
        Self {
            conn,
            keys: Arc::default(),
        }
    }

    /// Load the scripts a push and a cancel run, so the first of each is one
    /// round trip rather than a refused call, a load and the call again.
    pub(crate) async fn load_scripts(&self) -> Result<(), redis::RedisError> {
        futures_util::future::try_join_all(SCRIPTS.producer().map(|script| self.conn.load(script)))
            .await
            .map(drop)
    }

    /// Cancel the job `job` names, or the one holding the unique key `key`.
    async fn cancel(&self, queue: &QueueName, job: &str, key: &str) -> Result<bool, QueueError> {
        let keys = self.keys.of(queue);
        let cancelled: i64 = self
            .conn
            .invoke(
                SCRIPTS
                    .cancel
                    .keys(&keys.transition()[..8])
                    .arg(GROUP)
                    .arg(job)
                    .arg(key),
            )
            .await
            .map_err(QueueError::backend)?;
        Ok(cancelled == 1)
    }
}

impl<I: Instance> nest_rs_core::CarriesRemedy for RedisQueueProducer<I> {
    fn remedy() -> Cow<'static, str> {
        // The port takes one binding, so the line holds beside one already
        // imported on another instance: it replaces, never adds.
        format!(
            "Bind the queue port on {} — `{}`, in place of any other queue binding — or push \
             jobs through the port's `Arc<nest_rs::queue::Jobs>`.",
            instance_phrase::<I>(),
            typed_import::<I>("nest_rs::redis::RedisQueueModule"),
        )
        .into()
    }
}

#[async_trait]
impl<I: Instance> JobProducer for RedisQueueProducer<I> {
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
        let mut push = SCRIPTS.push.keys(&keys.transition()[..7]);
        push.arg(after);
        let pushed = envelopes.len();
        for envelope in envelopes {
            let id = envelope.id().to_string();
            let unique = envelope.unique_key().unwrap_or_default().to_owned();
            let record = serde_json::to_vec(&envelope.into_json())?;
            push.arg(id).arg(record).arg(unique);
        }
        let answer: redis::Value = self.conn.invoke(&push).await.map_err(QueueError::backend)?;
        match answer {
            redis::Value::Int(0) => Ok(()),
            redis::Value::Array(refused) => {
                let held = redis::from_redis_value::<(usize, String)>(redis::Value::Array(refused))
                    .ok()
                    .filter(|(at, _)| *at < pushed)
                    .map(|(_, holder)| holder);
                let Some(holder) = held else {
                    return Err(QueueError::backend(UnexpectedReply {
                        call: "the push",
                        expected: "the position of a job pushed, and a job id",
                    }));
                };
                Err(QueueError::UniqueKeyHeld {
                    queue: queue.clone(),
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
