//! [`Promotion`] — one queue's due records, moved from apalis's schedule onto
//! the list a worker fetches from: the step both halves of a queue take, the
//! producer for the delayed records it filed, and the worker for every record of
//! its queue that falls due.
//!
//! apalis keeps a record held back — a delayed push, a retry's next attempt, a
//! job handed back — on the queue's schedule, and only its public
//! `enqueue_scheduled` moves a due one onto `nestrs:queue:<queue>:active`, the
//! list a worker fetches from and an autoscaler reads. One scan is one script,
//! moving up to a batch of due records, earliest due first; a scan every
//! [`TICK`] moves a batch a second for as long as records are due.

use std::time::Duration;

use apalis_redis::RedisStorage;
use nest_rs_queue::QueueName;

use crate::RedisConnection;
use crate::layout;

/// How often a promotion scans: once a second, the resolution apalis schedules
/// on.
pub(crate) const TICK: Duration = Duration::from_secs(1);

/// How many due records a scan moves: a hundred — the producer's batch, and a
/// worker's unless its method runs more jobs at once.
pub(crate) const BATCH: usize = 100;

/// The longest a promotion backs off to while Redis refuses its scan.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// One queue's promotion: its schedule, how many due records a scan moves, and
/// how long until the next scan.
pub(crate) struct Promotion {
    storage: RedisStorage<serde_json::Value, RedisConnection>,
    queue: QueueName,
    batch: usize,
    wait: Duration,
}

impl Promotion {
    /// The promotion of `queue` over `conn`, moving up to `batch` due records a
    /// scan.
    pub(crate) fn new(conn: &RedisConnection, queue: &QueueName, batch: usize) -> Self {
        Self {
            storage: RedisStorage::new_with_config(conn.clone(), layout::config(queue)),
            queue: queue.clone(),
            batch,
            wait: TICK,
        }
    }

    /// How long until the next scan: a tick, or — while Redis refuses the
    /// scan — a backoff doubling up to thirty seconds.
    pub(crate) fn wait(&self) -> Duration {
        self.wait
    }

    /// Move up to a batch of due records onto the queue: how many moved, or
    /// `None` when Redis refused the scan, which is said at `warn` with when the
    /// next one runs.
    pub(crate) async fn scan(&mut self) -> Option<usize> {
        match self.storage.enqueue_scheduled(self.batch).await {
            Ok(moved) => {
                self.wait = TICK;
                if moved > 0 {
                    tracing::debug!(
                        target: nest_rs_queue::TARGET,
                        queue = %self.queue,
                        moved,
                        "due jobs moved onto the queue",
                    );
                }
                Some(moved)
            }
            Err(error) => {
                self.wait = (self.wait * 2).min(MAX_BACKOFF);
                report_scan_failure(&self.queue, self.wait, &error);
                None
            }
        }
    }
}

/// A scan Redis refused, said at `warn` with when the next one runs: the due
/// jobs it would have moved wait until then — or until another side's scan.
fn report_scan_failure(queue: &QueueName, retry_in: Duration, error: &redis::RedisError) {
    tracing::warn!(
        target: nest_rs_queue::TARGET,
        queue = %queue,
        retry_in_ms = u64::try_from(retry_in.as_millis()).unwrap_or(u64::MAX),
        error = %nest_rs_core::error_message(error),
        "due jobs not moved onto the queue; retrying",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "due jobs not moved onto the queue; retrying",
        );
        assert_eq!(refused.level, "warn");
        assert_eq!(refused.field("queue").as_deref(), Some("audio"));
        assert_eq!(refused.field("retry_in_ms").as_deref(), Some("2000"));
        assert_eq!(refused.field("error").as_deref(), Some("timed out"));
    }
}
