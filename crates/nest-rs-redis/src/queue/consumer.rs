//! [`RedisQueueConsumer`] — the queue port's consumer half over Redis Streams:
//! the [`JobConsumer`] the port's `QueueWorker` runs every `#[process]` method
//! through.
//!
//! **A delivery is a pending entry.** `XREADGROUP` hands an entry over, records
//! this worker as its owner and starts its idle clock in one command, so no
//! job is ever handed over without a lease. The worker renews the lease by
//! claiming the entry again without counting a delivery; once no renewal came
//! for a whole lease, the next worker that asks takes it, which counts one. A
//! read blocks on a connection of its own, since on the shared socket it would
//! stall every other caller.
//!
//! **What a delivery writes, it writes fenced** ([`super::scripts`]), so a
//! worker whose lease lapsed under it — frozen, or cut off from Redis — writes
//! nothing once another holds the job.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use nest_rs_queue::{
    Ask, CheckpointStore, Delivery, Disposition, JobConsumer, JobId, LeaseHold, Prepared,
    ProcessMethod, QueueBackend, QueueError, Received,
};
use redis::Value;
use tokio::sync::Mutex as AsyncMutex;

use super::backend::BACKEND;
use super::checkpoint::RedisCheckpoint;
use super::error::{PreparedTwice, UnexpectedReply, UnknownDisposition};
use super::layout::{DEAD_KEPT, DEAD_MOST, GROUP, QueueKeys};
use super::scripts::SCRIPTS;
use crate::RedisConnection;
use crate::millis::millis;

/// The most held-back jobs one upkeep files: each is a write a script holds
/// Redis for, so a backlog falling due at once is filed in steps.
const PROMOTE_BATCH: u32 = 100;

/// The longest between two upkeeps of a queue: a delayed job pushed by another
/// replica is filed within it of falling due.
const UPKEEP_MOST: Duration = Duration::from_secs(1);

/// How often a queue's lapsed leases are looked for, at most: a look costs a
/// bounded read of the deliveries running, and a lease lapses on a scale of
/// seconds.
const RECLAIM_EVERY: Duration = Duration::from_secs(1);

/// The most pending entries a look reads into its script — `XAUTOCLAIM`'s own
/// scan bound at its default `COUNT` (100, scanning ten times it).
const RECLAIM_PAGE: u32 = 1000;

/// The longest pending list a look reads whole, Redis filtering the lapsed
/// entries itself: measured on Redis 8.6, it filters ten entries for what one
/// read into the script costs, so a whole list this long costs a page, and a
/// longer one is read a page per look — no look costs more, however long.
const RECLAIM_WHOLE: u32 = 10 * RECLAIM_PAGE;

/// How often a queue's group is swept of the consumers stopped replicas left.
const SWEEP_EVERY: Duration = Duration::from_secs(60);

/// How long a consumer holding nothing stays silent before it is swept: a live
/// worker reads its queue every second, so an hour of silence is a replica
/// gone.
const SILENT_FOR: Duration = Duration::from_secs(60 * 60);

/// What a delivery holds its job by: its entry, the delivery count it was
/// handed over with, and the job's id.
#[derive(Clone, Debug)]
pub(crate) struct Lease {
    pub(crate) entry: String,
    pub(crate) count: u32,
    pub(crate) job: String,
}

/// One queue this worker drains: its keys, the connection its blocking read
/// waits on, and when its upkeeps last ran.
struct Drained {
    keys: QueueKeys,
    reading: AsyncMutex<Option<RedisConnection>>,
    reclaimed: Mutex<Option<Instant>>,
    /// Where the next look for lapsed leases starts: `-`, or `(` and the last
    /// entry the previous look read.
    reclaim_from: Mutex<String>,
    swept: Mutex<Option<Instant>>,
}

impl Drained {
    /// Whether `every` has passed since `last`, marking now when it has.
    fn due(last: &Mutex<Option<Instant>>, every: Duration) -> bool {
        let mut last = last.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        if last.is_some_and(|at| now.duration_since(at) < every) {
            return false;
        }
        *last = Some(now);
        true
    }
}

/// The Redis queue's consumer, bound by
/// [`RedisQueueModule`](crate::RedisQueueModule) for the port's worker.
pub(crate) struct RedisQueueConsumer {
    conn: RedisConnection,
    lease: Duration,
    /// This worker's consumer name in every queue's group: a UUID v7, which
    /// also says when it started.
    name: String,
    /// Each queue drained, by the name of the method draining it — set once by
    /// `prepare`.
    drained: OnceLock<HashMap<&'static str, Arc<Drained>>>,
}

impl RedisQueueConsumer {
    pub(crate) fn new(conn: RedisConnection, lease: Duration) -> Self {
        Self {
            conn,
            lease,
            name: uuid::Uuid::now_v7().to_string(),
            drained: OnceLock::new(),
        }
    }

    fn drained(&self, method: &'static ProcessMethod) -> Result<&Arc<Drained>, QueueError> {
        self.drained
            .get()
            .and_then(|drained| drained.get(method.name()))
            .ok_or_else(|| {
                QueueError::backend(UnexpectedReply {
                    call: "a queue the worker did not prepare",
                    expected: "a method prepared at boot",
                })
            })
    }

    /// Up to `max` deliveries whose lease lapsed, taken for this worker.
    async fn reclaim(
        &self,
        method: &'static ProcessMethod,
        drained: &Drained,
        max: u32,
    ) -> Result<Vec<Delivery<Lease>>, QueueError> {
        let from = drained
            .reclaim_from
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let (taken, vanished, next): (Vec<Value>, Vec<String>, String) = self
            .conn
            .invoke(
                SCRIPTS
                    .reclaim
                    .keys(&[&drained.keys.jobs])
                    .arg(GROUP)
                    .arg(&self.name)
                    .arg(millis(self.lease))
                    .arg(max)
                    .arg(from)
                    .arg(RECLAIM_PAGE)
                    .arg(RECLAIM_WHOLE),
            )
            .await
            .map_err(QueueError::backend)?;
        *drained
            .reclaim_from
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = if next == "-" {
            next
        } else {
            format!("({next}")
        };
        said_vanished(method, &vanished);
        taken
            .into_iter()
            .map(|taken| {
                let Value::Array(mut parts) = taken else {
                    return Err(reclaim_shape());
                };
                let (Some(fields), Some(count), Some(entry)) =
                    (parts.pop(), parts.pop(), parts.pop())
                else {
                    return Err(reclaim_shape());
                };
                let entry: String =
                    redis::from_redis_value(entry).map_err(|_shape| reclaim_shape())?;
                let count: u32 =
                    redis::from_redis_value(count).map_err(|_shape| reclaim_shape())?;
                Ok(self.delivery(entry, count, fields))
            })
            .collect()
    }

    /// Up to `max` deliveries never handed over before, waiting `wait` for the
    /// first on the queue's own connection.
    async fn read(
        &self,
        drained: &Drained,
        max: u32,
        wait: Duration,
    ) -> Result<Vec<Delivery<Lease>>, QueueError> {
        let mut reading = drained.reading.lock().await;
        let conn = match reading.as_ref() {
            Some(conn) => conn,
            None => reading.insert(
                self.conn
                    .with_budget(wait + self.conn.budget())
                    .dedicated(&drained.keys.jobs)
                    .await
                    .map_err(QueueError::backend)?,
            ),
        };
        let mut conn = conn.with_budget(wait + self.conn.budget());
        drop(reading);
        let read: Result<Value, redis::RedisError> = redis::cmd("XREADGROUP")
            .arg("GROUP")
            .arg(GROUP)
            .arg(&self.name)
            .arg("COUNT")
            .arg(max)
            .arg("BLOCK")
            .arg(millis(wait))
            .arg("STREAMS")
            .arg(&drained.keys.jobs)
            .arg(">")
            .query_async(&mut conn)
            .await;
        let read = match read {
            Ok(read) => read,
            // The stream or its group went — deleted by hand, or flushed: the
            // group is made again, from the stream's first entry, and the next
            // read finds what was filed meanwhile.
            Err(error) if error.code() == Some("NOGROUP") => {
                create_group(&mut self.conn.clone(), &drained.keys).await?;
                return Ok(Vec::new());
            }
            // The link that met it is given up: the next read opens another,
            // which on a Cluster asks again which node serves the queue.
            Err(error) => {
                *drained.reading.lock().await = None;
                return Err(QueueError::backend(error));
            }
        };
        entries(read)?
            .into_iter()
            .map(|(entry, fields)| Ok(self.delivery(entry, 1, fields)))
            .collect()
    }

    /// The delivery of `entry`, handed over for the `count`th time, from its
    /// stream fields.
    fn delivery(&self, entry: String, count: u32, fields: Value) -> Delivery<Lease> {
        let mut job = None;
        let mut record = None;
        let mut deferred = None;
        if let Value::Array(fields) = fields {
            let mut fields = fields.into_iter();
            while let (Some(name), Some(value)) = (fields.next(), fields.next()) {
                if let (Value::BulkString(name), Value::BulkString(value)) = (name, value) {
                    match name.as_slice() {
                        b"job" => job = String::from_utf8(value).ok(),
                        b"record" => record = Some(value),
                        b"deferred" => {
                            deferred = std::str::from_utf8(&value)
                                .ok()
                                .and_then(|at| at.parse::<u64>().ok());
                        }
                        _ => {}
                    }
                }
            }
        }
        let lease = Lease {
            entry,
            count,
            job: job.unwrap_or_default(),
        };
        let backend_id = lease.entry.clone();
        // How long the job has waited for a release that reads it: from its
        // first hand-back to its filing on the stream, both on Redis's clock.
        let waited = deferred
            .zip(entry_millis(&lease.entry))
            .map(|(first, filed)| Duration::from_millis(filed.saturating_sub(first)));
        let delivery = match record {
            Some(record) => Delivery::new(record, lease, self.lease),
            None => Delivery::refused(lease, self.lease, "the stream entry carries no record"),
        };
        let delivery = delivery
            .with_delivery_count(count)
            .with_backend_id(backend_id);
        match waited {
            Some(waited) => delivery.with_deferred_for(waited),
            None => delivery,
        }
    }
}

impl JobConsumer for RedisQueueConsumer {
    type Lease = Lease;

    fn backend(&self) -> &'static QueueBackend {
        &BACKEND
    }

    async fn prepare(&self, methods: &[&'static ProcessMethod]) -> Result<Prepared, QueueError> {
        let mut conn = self.conn.clone();
        let mut drained = HashMap::with_capacity(methods.len());
        for method in methods {
            let queue = nest_rs_queue::QueueName::new(method.queue())?;
            let keys = QueueKeys::new(&queue);
            create_group(&mut conn, &keys).await?;
            drained.insert(
                method.name(),
                Arc::new(Drained {
                    keys,
                    reading: AsyncMutex::new(None),
                    reclaimed: Mutex::new(None),
                    reclaim_from: Mutex::new(String::from("-")),
                    swept: Mutex::new(None),
                }),
            );
        }
        futures_util::future::try_join_all(SCRIPTS.consumer().map(|script| self.conn.load(script)))
            .await
            .map_err(QueueError::backend)?;
        if self.drained.set(drained).is_err() {
            return Err(QueueError::backend(PreparedTwice));
        }
        Ok(Prepared::new(self.conn.budget()))
    }

    async fn receive(
        &self,
        method: &'static ProcessMethod,
        ask: Ask,
    ) -> Result<Received<Lease>, QueueError> {
        let drained = self.drained(method)?;
        let mut max = ask.max.get();
        let mut admitted = None;
        if let Some(throttle) = method.options().throttle() {
            let (taken, closed_for): (u32, u64) = self
                .conn
                .invoke(
                    SCRIPTS
                        .admit
                        .keys(&[&drained.keys.throttle])
                        .arg(throttle.limit().get())
                        .arg(millis(throttle.window()))
                        .arg(max),
                )
                .await
                .map_err(QueueError::backend)?;
            if taken == 0 {
                return Ok(
                    Received::new(Vec::new()).throttled_for(Duration::from_millis(closed_for))
                );
            }
            max = taken;
            admitted = Some(taken);
        }
        let mut deliveries = if Drained::due(&drained.reclaimed, RECLAIM_EVERY) {
            self.reclaim(method, drained, max).await?
        } else {
            Vec::new()
        };
        if deliveries.is_empty() {
            deliveries = self.read(drained, max, ask.wait).await?;
        }
        if let Some(admitted) = admitted {
            let unused =
                admitted.saturating_sub(u32::try_from(deliveries.len()).unwrap_or(u32::MAX));
            if unused > 0 {
                // Starts counted and not used are given back while their window
                // lasts; a give-back that fails only throttles more, never less.
                let given: Result<i64, _> = self
                    .conn
                    .invoke(SCRIPTS.release.keys(&[&drained.keys.throttle]).arg(unused))
                    .await;
                if let Err(error) = given {
                    tracing::debug!(
                        target: nest_rs_queue::TARGET,
                        queue = method.queue(),
                        unused,
                        error = %nest_rs_core::error_message(&error),
                        "throttle starts counted and unused not given back; the window throttles them",
                    );
                }
            }
        }
        Ok(Received::new(deliveries))
    }

    async fn renew(
        &self,
        method: &'static ProcessMethod,
        leases: &[&Lease],
    ) -> Result<Vec<LeaseHold>, QueueError> {
        if leases.is_empty() {
            return Ok(Vec::new());
        }
        let drained = self.drained(method)?;
        let mut renew = SCRIPTS.renew.keys(&[&drained.keys.jobs]);
        renew.arg(GROUP).arg(&self.name);
        for lease in leases {
            renew.arg(&lease.entry).arg(lease.count);
        }
        let held: Vec<i64> = self
            .conn
            .invoke(&renew)
            .await
            .map_err(QueueError::backend)?;
        if held.len() != leases.len() {
            return Err(QueueError::backend(UnexpectedReply {
                call: "the renewal",
                expected: "one answer per lease",
            }));
        }
        let vanished: Vec<&str> = held
            .iter()
            .zip(leases)
            .filter(|(answer, _)| **answer == 2)
            .map(|(_, lease)| lease.entry.as_str())
            .collect();
        said_vanished(method, &vanished);
        Ok(held.into_iter().map(hold).collect())
    }

    async fn settle(
        &self,
        method: &'static ProcessMethod,
        lease: &Lease,
        disposition: Disposition<'_>,
    ) -> Result<LeaseHold, QueueError> {
        let wait = |after: Duration| if after.is_zero() { 0 } else { millis(after) };
        let (way, after, record, reason): (&str, u64, &[u8], &str) = match disposition {
            Disposition::Complete => ("complete", 0, &[], ""),
            Disposition::Retry { after, record, .. } => ("retry", wait(after), record, ""),
            Disposition::Defer { after, record, .. } => ("defer", wait(after), record, ""),
            Disposition::Requeue { record, .. } => ("requeue", 0, record, ""),
            Disposition::DeadLetter { reason, record, .. } => ("dead", 0, record, reason),
            _ => return Err(QueueError::backend(UnknownDisposition)),
        };
        let keys = &self.drained(method)?.keys;
        let settled: i64 = self
            .conn
            .invoke(
                SCRIPTS
                    .settle
                    .keys(&keys.transition())
                    .arg(GROUP)
                    .arg(&self.name)
                    .arg(&lease.entry)
                    .arg(lease.count)
                    .arg(&lease.job)
                    .arg(way)
                    .arg(after)
                    .arg(record)
                    .arg(reason)
                    .arg(DEAD_MOST)
                    .arg(millis(DEAD_KEPT)),
            )
            .await
            .map_err(QueueError::backend)?;
        Ok(hold(settled))
    }

    async fn maintain(
        &self,
        method: &'static ProcessMethod,
    ) -> Result<Option<Duration>, QueueError> {
        let drained = self.drained(method)?;
        let keys = &drained.keys;
        let (next, lost): (i64, Vec<String>) = self
            .conn
            .invoke(
                SCRIPTS
                    .promote
                    .keys(&keys.transition())
                    .arg(PROMOTE_BATCH)
                    .arg(millis(DEAD_KEPT)),
            )
            .await
            .map_err(QueueError::backend)?;
        said_lost(method, &lost);
        if Drained::due(&drained.swept, SWEEP_EVERY) {
            let swept: i64 = self
                .conn
                .invoke(
                    SCRIPTS
                        .sweep
                        .keys(&[&keys.jobs])
                        .arg(GROUP)
                        .arg(millis(SILENT_FOR)),
                )
                .await
                .map_err(QueueError::backend)?;
            if swept > 0 {
                tracing::debug!(
                    target: nest_rs_queue::TARGET,
                    queue = method.queue(),
                    swept,
                    "consumers of stopped workers removed from the queue's group",
                );
            }
        }
        let next = u64::try_from(next).map_or(UPKEEP_MOST, Duration::from_millis);
        Ok(Some(next.min(UPKEEP_MOST)))
    }

    async fn close(&self) -> Result<(), QueueError> {
        let Some(drained) = self.drained.get() else {
            return Ok(());
        };
        // Every queue is left, whichever failed: one left behind is swept once
        // silent, and the first failure is the one answered.
        let mut failed = None;
        for queue in drained.values() {
            let left: Result<i64, _> = self
                .conn
                .invoke(
                    SCRIPTS
                        .leave
                        .keys(&[&queue.keys.jobs])
                        .arg(GROUP)
                        .arg(&self.name),
                )
                .await;
            if let Err(error) = left {
                failed.get_or_insert(error);
            }
        }
        failed.map_or(Ok(()), |error| Err(QueueError::backend(error)))
    }

    fn checkpoint(
        &self,
        method: &'static ProcessMethod,
        lease: &Lease,
        job: &JobId,
    ) -> Option<Arc<dyn CheckpointStore>> {
        let keys = &self.drained(method).ok()?.keys;
        Some(Arc::new(RedisCheckpoint::new(
            self.conn.clone(),
            keys,
            self.name.clone(),
            lease.clone(),
            job.to_string(),
        )))
    }
}

/// Make `keys`' group, from the stream's first entry — so jobs pushed before
/// any worker started are read — unless it is there already.
async fn create_group(conn: &mut RedisConnection, keys: &QueueKeys) -> Result<(), QueueError> {
    let created: Result<(), redis::RedisError> = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(&keys.jobs)
        .arg(GROUP)
        .arg("0")
        .arg("MKSTREAM")
        .query_async(conn)
        .await;
    match created {
        Err(error) if error.code() != Some("BUSYGROUP") => Err(QueueError::backend(error)),
        _ => Ok(()),
    }
}

/// The entries an `XREADGROUP` of one stream answered: none on a timeout.
fn entries(read: Value) -> Result<Vec<(String, Value)>, QueueError> {
    let shape = || {
        QueueError::backend(UnexpectedReply {
            call: "the read",
            expected: "nil, or one stream's entries",
        })
    };
    let streams = match read {
        Value::Nil => return Ok(Vec::new()),
        Value::Array(streams) => streams,
        _ => return Err(shape()),
    };
    let mut found = Vec::new();
    for stream in streams {
        let Value::Array(mut stream) = stream else {
            return Err(shape());
        };
        let Some(Value::Array(read)) = stream.pop() else {
            return Err(shape());
        };
        for entry in read {
            let Value::Array(mut entry) = entry else {
                return Err(shape());
            };
            let (Some(fields), Some(id)) = (entry.pop(), entry.pop()) else {
                return Err(shape());
            };
            let id: String = redis::from_redis_value(id).map_err(|_shape| shape())?;
            found.push((id, fields));
        }
    }
    Ok(found)
}

/// The millisecond an entry id was filed at, on Redis's clock.
fn entry_millis(entry: &str) -> Option<u64> {
    entry.split_once('-').and_then(|(at, _)| at.parse().ok())
}

/// Say the entries of `method`'s queue deleted while a delivery held them —
/// by hand, since no script deletes one still pending: their jobs are gone.
fn said_vanished(method: &'static ProcessMethod, entries: &[impl AsRef<str>]) {
    if entries.is_empty() {
        return;
    }
    let backend_ids: Vec<&str> = entries.iter().map(AsRef::as_ref).collect();
    tracing::warn!(
        target: nest_rs_queue::TARGET,
        queue = method.queue(),
        vanished = backend_ids.len(),
        backend_ids = %backend_ids.join(","),
        "queue entries deleted while delivered; their jobs are gone, and a unique key they \
         held stays held until cancel_unique frees it",
    );
}

/// Say the held-back jobs of `method`'s queue found due without their record —
/// deleted by hand, since no script deletes one a job still waits on: they are
/// gone, and the upkeep let go of what they held.
fn said_lost(method: &'static ProcessMethod, jobs: &[String]) {
    if jobs.is_empty() {
        return;
    }
    tracing::warn!(
        target: nest_rs_queue::TARGET,
        queue = method.queue(),
        lost = jobs.len(),
        job_ids = %jobs.join(","),
        "held-back queue jobs found due without their record are gone; what they held is let go",
    );
}

/// A fenced script's `1` or `0`.
fn hold(answer: i64) -> LeaseHold {
    if answer == 1 {
        LeaseHold::Held
    } else {
        LeaseHold::Lost
    }
}

fn reclaim_shape() -> QueueError {
    QueueError::backend(UnexpectedReply {
        call: "the reclaim",
        expected: "an entry, its delivery count and its fields",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bulk(text: &str) -> Value {
        Value::BulkString(text.as_bytes().to_vec())
    }

    #[test]
    fn a_read_answers_its_entries_or_none() {
        assert!(entries(Value::Nil).expect("a timeout").is_empty());
        let read = Value::Array(vec![Value::Array(vec![
            bulk("nestrs:queue:{audio}:jobs"),
            Value::Array(vec![Value::Array(vec![
                bulk("1700000000000-0"),
                Value::Array(vec![
                    bulk("job"),
                    bulk("01890a5d-ac96-774b-bcce-b302099a8057"),
                ]),
            ])]),
        ])]);
        let found = entries(read).expect("one entry");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "1700000000000-0");
        assert!(entries(Value::Int(1)).is_err());
    }

    #[test]
    fn an_entry_id_says_when_it_was_filed() {
        assert_eq!(entry_millis("1700000000123-4"), Some(1_700_000_000_123));
        assert_eq!(entry_millis("not-an-id"), None);
    }
}
