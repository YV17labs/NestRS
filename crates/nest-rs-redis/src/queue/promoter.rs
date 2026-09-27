//! [`Promoter`] — the producer's half of delayed delivery: it moves the delayed
//! records it filed onto their queue once they are due, so a job held back
//! becomes visible in `nestrs:queue:<queue>:active` whether or not a worker is
//! running.
//!
//! apalis keeps a delayed record on the queue's schedule, and only its
//! `enqueue_scheduled` moves a due one onto the list a worker fetches from — a
//! [`Promotion`] every running worker runs each second. That list is also what
//! an autoscaler reads (KEDA's `redis` trigger polls its length), so with no
//! worker running a due job would never appear there, and the autoscaler waiting
//! for it would never start the worker that could have moved it: a deployment
//! scaled to zero would hold a delayed job forever. So the producer that filed it
//! moves it too, on the same one-second tick and a hundred at a time, for as long
//! as it has delayed records of its own still ahead of it; the tick stops by
//! itself once the last of them is due and nothing due is left behind.
//!
//! **What it does not cover** is a producer that exits before its records are
//! due. They stay on the schedule until a worker or a producer scans it again,
//! which is why a deployment with no producer process running keeps at least one
//! worker replica (`minReplicaCount: 1`) — the queue documentation says so where
//! it shows the trigger.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use nest_rs_queue::QueueName;

use crate::RedisConnection;
use crate::promotion::{BATCH, Promotion};

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
        tokio::spawn(scan(
            self.clone(),
            Promotion::new(conn, queue, BATCH),
            queue.clone(),
        ));
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

/// Move `queue`'s due records onto its list at every tick until the last
/// record this producer filed is due and a scan found fewer than a batch to
/// move — nothing due is then left behind.
async fn scan(promoter: Promoter, mut promotion: Promotion, queue: QueueName) {
    loop {
        tokio::time::sleep(promotion.wait()).await;
        let now = now_second();
        if let Some(moved) = promotion.scan().await
            && moved < BATCH
            && promoter.done_with(&queue, now)
        {
            return;
        }
    }
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
}
