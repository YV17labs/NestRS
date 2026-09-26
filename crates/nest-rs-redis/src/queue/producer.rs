//! [`RedisQueueProducer`] — the queue port's producer half over the shared
//! [`RedisConnection`]: the [`JobProducer`] binding a feature injects as
//! `Arc<dyn JobProducer>` to push without naming the backend.
//!
//! The port checks the queue and the options, mints each job's id and seals
//! every job before this type sees it; what is stored is that sealed envelope,
//! as the port handed it, on an apalis `RedisStorage<serde_json::Value>` under
//! the queue's namespace (`nestrs:queue:<queue>`, see [`crate::layout`]) —
//! which is how apalis routes a job to the worker draining that queue. Every job
//! is a JSON `Value` at this boundary, so the worker never names the payload
//! type. The receipt a caller gets names the port's id; apalis's own task id for
//! the record stays inside this crate, and reaches a job's lines only as its
//! `backend_id`.
//!
//! A push holding the job back files it on the queue's schedule instead of its
//! list, due on the second its delay ends, and the producer's [`Promoter`]
//! moves it onto the list once due.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use apalis::prelude::{Request, Storage};
use apalis_redis::RedisStorage;
use async_trait::async_trait;
use nest_rs_queue::{Envelope, JobProducer, PushOptions, QueueBackend, QueueError, QueueName};

use super::promoter::Promoter;
use crate::RedisConnection;
use crate::backend::{BACKEND, due_second};
use crate::layout;

/// The producer a feature pushes through. Bound by
/// [`RedisQueueModule`](crate::RedisQueueModule) under both its own name and
/// `Arc<dyn JobProducer>`; a `Clone` shares the underlying connection, the
/// delayed records it watches and the queues it has checked.
///
/// It declares delayed delivery and no other optional capability (see the
/// crate's backend declaration), so a unique key and a cancel are refused by
/// the port before they reach it, naming this backend.
#[derive(Clone)]
pub struct RedisQueueProducer {
    conn: RedisConnection,
    promoter: Promoter,
    /// The queues whose 6.x keys this producer has looked for — once per queue
    /// per process, so the check costs one round trip the first time and never
    /// again.
    checked: Arc<Mutex<HashSet<QueueName>>>,
}

impl RedisQueueProducer {
    /// A producer over the app's shared connection (reused, never reopened).
    pub fn new(conn: RedisConnection) -> Self {
        Self {
            conn,
            promoter: Promoter::default(),
            checked: Arc::default(),
        }
    }

    /// Producer-side storage handle under `queue`'s namespace, the one the
    /// worker draining it reads.
    fn storage(&self, queue: &QueueName) -> RedisStorage<serde_json::Value, RedisConnection> {
        RedisStorage::new_with_config(self.conn.clone(), layout::config(queue))
    }

    /// Say once per queue when jobs still wait under the 6.x layout: this push
    /// lands where a 7.0 worker reads, and those do not. The worker refuses to
    /// start beside them; a producer has nothing to refuse — its push is right —
    /// so it warns, and keeps pushing.
    async fn look_for_legacy_jobs(&self, queue: &QueueName) {
        let first = self
            .checked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(queue.clone());
        if !first {
            return;
        }
        report_legacy_check(queue, layout::legacy_jobs(&self.conn, queue).await);
    }
}

/// What the producer's one look at the 6.x layout says: jobs waiting there at
/// `warn`, since no 7.0 worker will run them; a check the ACL refused as detail,
/// since a user confined to the framework's keys could not have written them;
/// any other refusal at `warn`, since those jobs would then go unnoticed.
fn report_legacy_check(queue: &QueueName, outcome: Result<Vec<String>, redis::RedisError>) {
    match outcome {
        Ok(keys) if keys.is_empty() => {}
        Ok(keys) => tracing::warn!(
            target: nest_rs_queue::TARGET,
            queue = %queue,
            keys = %keys.join(", "),
            namespace = %layout::namespace(queue),
            "jobs wait under the 6.x key layout; a 7.0 worker does not run them — drain them \
             with a 6.x worker, or move them under the queue's namespace",
        ),
        Err(error) if layout::outside_the_acl(&error) => tracing::debug!(
            target: nest_rs_queue::TARGET,
            queue = %queue,
            error = %nest_rs_core::error_message(&error),
            "6.x key layout not checked: the connection's ACL does not reach it",
        ),
        Err(error) => tracing::warn!(
            target: nest_rs_queue::TARGET,
            queue = %queue,
            error = %nest_rs_core::error_message(&error),
            "6.x key layout not checked; jobs waiting there would go unnoticed",
        ),
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
        self.look_for_legacy_jobs(queue).await;
        // `push` takes `&mut self`; storage is a cheap clone of the connection
        // handle, so build one per call rather than force callers to hold it mut.
        let mut storage = self.storage(queue);
        let now = SystemTime::now();
        let due = match options.delay() {
            None => None,
            Some(delay) => match delay.deadline(now) {
                // An instant already past is an immediate push.
                Some(at) if at <= now => None,
                Some(at) => Some(due_second(at)),
                None => {
                    return Err(QueueError::InvalidOptions {
                        reason: "the delay ends past the last instant the clock can represent",
                    });
                }
            },
        };
        match due {
            None => {
                for envelope in envelopes {
                    storage
                        .push(envelope.into_json())
                        .await
                        .map_err(QueueError::backend)?;
                }
            }
            Some(second) => {
                for envelope in envelopes {
                    storage
                        .schedule_request(Request::new(envelope.into_json()), second)
                        .await
                        .map_err(QueueError::backend)?;
                }
                self.promoter.watch(&self.conn, queue, second);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_testing::LogCapture;

    use super::*;

    /// A check Redis refused for any reason but the ACL is a `warn`: the jobs it
    /// was looking for would otherwise wait with nothing to say so. One the ACL
    /// refused is detail, and a check that found nothing says nothing.
    #[test]
    fn a_6x_check_redis_refuses_is_said_and_one_the_acl_refuses_is_detail() {
        let logs = LogCapture::install();
        let audio = QueueName::new("audio").expect("a valid name");
        report_legacy_check(&audio, Ok(Vec::new()));
        report_legacy_check(
            &audio,
            Err(redis::RedisError::from(std::io::Error::other("timed out"))),
        );
        report_legacy_check(
            &audio,
            Err(redis::make_extension_error(
                "NOPERM".to_owned(),
                Some("this user has no permissions to access one of the keys".to_owned()),
            )),
        );
        let unchecked = logs.expect_one(
            nest_rs_queue::TARGET,
            "6.x key layout not checked; jobs waiting there would go unnoticed",
        );
        assert_eq!(unchecked.level, "warn");
        assert_eq!(unchecked.field("error").as_deref(), Some("timed out"));
        assert_eq!(logs.events().len(), 2, "the empty check said nothing");
    }
}
