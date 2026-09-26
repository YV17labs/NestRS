//! [`Promoter`] — the producer's half of delayed delivery: it moves the delayed
//! records it filed onto their queue once they are due, so a job held back
//! becomes visible in `nestrs:queue:<queue>:active` whether or not a worker is
//! running.
//!
//! apalis keeps a delayed record on the queue's schedule, and only its
//! `enqueue_scheduled` moves a due one onto the list a worker fetches from — a
//! call its own worker makes on a heartbeat. That list is also what an
//! autoscaler reads (KEDA's `redis` trigger polls its length), so with no worker
//! running a due job would never appear there, and the autoscaler waiting for it
//! would never start the worker that could have moved it: a deployment scaled to
//! zero would hold a delayed job forever. So the producer that filed it moves it
//! too, on a one-second tick, for as long as it has delayed records of its own
//! still ahead of it; the tick stops by itself once the last of them is due and
//! nothing due is left behind.
//!
//! **What it does not cover** is a producer that exits before its records are
//! due. They stay on the schedule until a worker or a producer scans it again,
//! which is why a deployment with no producer process running keeps at least one
//! worker replica (`minReplicaCount: 1`) — the queue documentation says so where
//! it shows the trigger.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use apalis_redis::RedisStorage;
use nest_rs_queue::QueueName;

use crate::RedisConnection;
use crate::layout;

/// How often a producer with delayed records outstanding scans their schedule:
/// once a second, the resolution apalis schedules on.
const TICK: Duration = Duration::from_secs(1);

/// The most records one scan moves; a scan that moves this many runs again at
/// the next tick, however far its deadline.
const BATCH: usize = 100;

/// The longest a tick backs off to while Redis refuses the scan.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// The delayed records one producer has filed and not yet seen due, per queue:
/// the second the last of them is due. A queue in the map has a scan running.
#[derive(Clone, Default)]
pub(crate) struct Promoter {
    outstanding: Arc<Mutex<HashMap<QueueName, i64>>>,
}

impl Promoter {
    /// A record due at `second` was filed on `queue`: scan the queue's schedule
    /// until then — starting a scan when none is running.
    pub(crate) fn watch(&self, conn: &RedisConnection, queue: &QueueName, second: i64) {
        let mut outstanding = self
            .outstanding
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(last) = outstanding.get_mut(queue) {
            *last = (*last).max(second);
            return;
        }
        outstanding.insert(queue.clone(), second);
        tokio::spawn(scan(self.clone(), conn.clone(), queue.clone()));
    }

    /// Whether `queue` still has records ahead of `now` — and when it has none,
    /// forget it, so the next delayed push starts a scan of its own.
    fn done_with(&self, queue: &QueueName, now: i64) -> bool {
        let mut outstanding = self
            .outstanding
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match outstanding.get(queue) {
            Some(last) if *last > now => false,
            _ => {
                outstanding.remove(queue);
                true
            }
        }
    }
}

/// Move `queue`'s due records onto its list every [`TICK`] until the last
/// record this producer filed is due and a scan found nothing more to move.
async fn scan(promoter: Promoter, conn: RedisConnection, queue: QueueName) {
    let mut storage: RedisStorage<serde_json::Value, RedisConnection> =
        RedisStorage::new_with_config(conn, layout::config(&queue));
    let mut wait = TICK;
    loop {
        tokio::time::sleep(wait).await;
        let now = now_second();
        match storage.enqueue_scheduled(BATCH).await {
            Ok(moved) => {
                wait = TICK;
                if moved > 0 {
                    tracing::debug!(
                        target: nest_rs_queue::TARGET,
                        queue = %queue,
                        moved,
                        "due delayed jobs moved onto the queue",
                    );
                }
                if moved < BATCH && promoter.done_with(&queue, now) {
                    return;
                }
            }
            Err(error) => {
                wait = (wait * 2).min(MAX_BACKOFF);
                report_scan_failure(&queue, wait, &error);
            }
        }
    }
}

/// A scan Redis refused, said at `warn` with when the next one runs: the due
/// jobs it would have moved wait until then — or until a worker's own scan.
fn report_scan_failure(queue: &QueueName, retry_in: Duration, error: &redis::RedisError) {
    tracing::warn!(
        target: nest_rs_queue::TARGET,
        queue = %queue,
        retry_in_ms = u64::try_from(retry_in.as_millis()).unwrap_or(u64::MAX),
        error = %nest_rs_core::error_message(error),
        "due delayed jobs not moved onto the queue; retrying",
    );
}

/// The current second, as apalis's schedule counts it.
fn now_second() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A queue stays outstanding until the last record filed on it is due, and a
    /// later record extends it rather than starting a second scan.
    #[test]
    fn a_queue_is_done_with_once_its_last_record_is_due() {
        let promoter = Promoter::default();
        let audio = QueueName::new("audio").expect("a valid name");
        promoter
            .outstanding
            .lock()
            .expect("unpoisoned")
            .insert(audio.clone(), 100);
        assert!(!promoter.done_with(&audio, 99));
        assert!(promoter.done_with(&audio, 100));
        assert!(
            promoter.outstanding.lock().expect("unpoisoned").is_empty(),
            "forgotten, so the next delayed push starts a scan"
        );
        assert!(
            promoter.done_with(&audio, 0),
            "a queue not outstanding is done"
        );
    }

    /// A scan Redis refused is said at `warn`, with when the next one runs.
    #[test]
    fn a_scan_redis_refuses_is_said_with_the_next_try() {
        let logs = nest_rs_testing::LogCapture::install();
        let audio = QueueName::new("audio").expect("a valid name");
        report_scan_failure(
            &audio,
            Duration::from_secs(2),
            &redis::RedisError::from(std::io::Error::other("timed out")),
        );
        let refused = logs.expect_one(
            nest_rs_queue::TARGET,
            "due delayed jobs not moved onto the queue; retrying",
        );
        assert_eq!(refused.level, "warn");
        assert_eq!(refused.field("retry_in_ms").as_deref(), Some("2000"));
        assert_eq!(refused.field("error").as_deref(), Some("timed out"));
    }
}
