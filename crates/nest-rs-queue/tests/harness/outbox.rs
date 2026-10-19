//! An in-memory outbox store over the database port's seams: a test commits
//! records as a unit would, and reads back what the relay left.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::future::BoxFuture;
use nest_rs_database::{ClaimedRecord, OutboxClaim, OutboxError, OutboxRecord, OutboxStore};
use tokio::sync::Notify;
use tokio::time::Instant;

/// One store, shared by every clone.
#[derive(Clone)]
pub(crate) struct MemoryOutbox {
    inner: Arc<Inner>,
}

struct Inner {
    rows: Mutex<Rows>,
    written: Notify,
    armed: AtomicBool,
    /// Whether the store's table is there to arm.
    table: bool,
    /// How many arming attempts still fail as a lost connection does.
    arm_failures: AtomicU32,
    /// Whether a settle never answers, holding its claim until dropped.
    silent_settle: AtomicBool,
}

#[derive(Default)]
struct Rows {
    next_seq: i64,
    by_seq: BTreeMap<i64, Row>,
}

struct Row {
    record: OutboxRecord,
    attempts: u32,
    written_at: Instant,
    due_at: Instant,
    next_attempt_at: Instant,
    locked: bool,
}

/// What the store still keeps of one record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Kept {
    pub(crate) seq: i64,
    pub(crate) id: uuid::Uuid,
    pub(crate) attempts: u32,
    /// How long until a pass may claim it again.
    pub(crate) due_again_in: Duration,
}

impl MemoryOutbox {
    pub(crate) fn new() -> Self {
        Self::with_table(true)
    }

    /// A store whose table was never created.
    pub(crate) fn without_table() -> Self {
        Self::with_table(false)
    }

    fn with_table(table: bool) -> Self {
        Self {
            inner: Arc::new(Inner {
                rows: Mutex::new(Rows::default()),
                written: Notify::new(),
                armed: AtomicBool::new(false),
                table,
                arm_failures: AtomicU32::new(0),
                silent_settle: AtomicBool::new(false),
            }),
        }
    }

    pub(crate) fn is_armed(&self) -> bool {
        self.inner.armed.load(Ordering::SeqCst)
    }

    /// Fail the next `attempts` armings as a connection the server reset.
    pub(crate) fn fail_arming(&self, attempts: u32) {
        self.inner.arm_failures.store(attempts, Ordering::SeqCst);
    }

    /// Make every settle wait forever, its claim held until it is dropped.
    pub(crate) fn settle_silently(&self) {
        self.inner.silent_settle.store(true, Ordering::SeqCst);
    }

    /// What a unit writing `records` and committing leaves: the rows, and the
    /// relay woken.
    pub(crate) fn commit(&self, records: impl IntoIterator<Item = OutboxRecord>) {
        let now = Instant::now();
        {
            let mut rows = self.rows();
            for record in records {
                rows.next_seq += 1;
                let seq = rows.next_seq;
                let due_at = now + record.delay;
                rows.by_seq.insert(
                    seq,
                    Row {
                        record,
                        attempts: 0,
                        written_at: now,
                        due_at,
                        next_attempt_at: now,
                        locked: false,
                    },
                );
            }
        }
        self.inner.written.notify_one();
    }

    /// Every record still kept, oldest first.
    pub(crate) fn kept(&self) -> Vec<Kept> {
        let now = Instant::now();
        self.rows()
            .by_seq
            .iter()
            .map(|(seq, row)| Kept {
                seq: *seq,
                id: row.record.id,
                attempts: row.attempts,
                due_again_in: row.next_attempt_at.saturating_duration_since(now),
            })
            .collect()
    }

    fn rows(&self) -> std::sync::MutexGuard<'_, Rows> {
        self.inner
            .rows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl OutboxStore for MemoryOutbox {
    fn arm(&self) -> BoxFuture<'_, Result<(), OutboxError>> {
        let failing = self
            .inner
            .arm_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        let armed = if failing {
            Err(OutboxError::Database(Box::new(std::io::Error::other(
                "connection reset by peer",
            ))))
        } else if self.inner.table {
            self.inner.armed.store(true, Ordering::SeqCst);
            Ok(())
        } else {
            Err(OutboxError::Missing {
                table: "nestrs_outbox",
                remedy: "create it",
            })
        };
        Box::pin(std::future::ready(armed))
    }

    fn written(&self, sweep: Duration) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            drop(tokio::time::timeout(sweep, self.inner.written.notified()).await);
        })
    }

    fn claim(&self, limit: usize) -> BoxFuture<'_, Result<Box<dyn OutboxClaim>, OutboxError>> {
        let now = Instant::now();
        let mut rows = self.rows();
        let mut records = Vec::new();
        for (seq, row) in rows
            .by_seq
            .iter_mut()
            .filter(|(_, row)| !row.locked && row.next_attempt_at <= now)
            .take(limit)
        {
            row.locked = true;
            let mut record = row.record.clone();
            record.delay = row.due_at.saturating_duration_since(now);
            records.push(ClaimedRecord::new(
                *seq,
                record,
                row.attempts,
                now.saturating_duration_since(row.written_at),
            ));
        }
        drop(rows);
        let claim: Box<dyn OutboxClaim> = Box::new(MemoryClaim {
            store: self.clone(),
            records,
            settled: false,
        });
        Box::pin(std::future::ready(Ok(claim)))
    }
}

/// A pass's locked records; dropped unsettled, it unlocks them as they were.
struct MemoryClaim {
    store: MemoryOutbox,
    records: Vec<ClaimedRecord>,
    settled: bool,
}

impl MemoryClaim {
    fn unlock(&self) {
        let mut rows = self.store.rows();
        for claimed in &self.records {
            if let Some(row) = rows.by_seq.get_mut(&claimed.seq) {
                row.locked = false;
            }
        }
    }
}

impl OutboxClaim for MemoryClaim {
    fn records(&self) -> &[ClaimedRecord] {
        &self.records
    }

    fn settle(
        mut self: Box<Self>,
        done: &[i64],
        retry: &[(i64, Duration)],
    ) -> BoxFuture<'static, Result<(), OutboxError>> {
        if self.store.inner.silent_settle.load(Ordering::SeqCst) {
            return Box::pin(async move {
                let _held = self;
                std::future::pending().await
            });
        }
        let now = Instant::now();
        {
            let mut rows = self.store.rows();
            for seq in done {
                rows.by_seq.remove(seq);
            }
            for (seq, wait) in retry {
                if let Some(row) = rows.by_seq.get_mut(seq) {
                    row.attempts += 1;
                    row.next_attempt_at = now + *wait;
                }
            }
        }
        self.unlock();
        self.settled = true;
        Box::pin(std::future::ready(Ok(())))
    }
}

impl Drop for MemoryClaim {
    fn drop(&mut self) {
        if !self.settled {
            self.unlock();
        }
    }
}
