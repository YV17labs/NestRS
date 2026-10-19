use std::time::Duration;

use futures_util::future::BoxFuture;

use crate::error::OutboxError;

/// One job a unit's transaction keeps until it commits: written by the queue
/// port on the ambient executor ([`Executor::write_outbox`](crate::Executor::write_outbox)),
/// read back by its relay through an [`OutboxStore`], and opaque to this port,
/// which names no queue type.
///
/// It can grow within 7.x: build one through [`OutboxRecord::new`].
#[non_exhaustive]
#[derive(Clone, PartialEq, Eq)]
pub struct OutboxRecord {
    /// The name of the queue the job is filed on.
    pub queue: String,
    /// The id the push minted for the job, which keys it in the backend too.
    pub id: uuid::Uuid,
    /// The job as the queue backend will store it. A payload: never logged,
    /// never quoted in an error.
    pub sealed: String,
    /// The unique key the push declared, if any.
    pub unique_key: Option<String>,
    /// How long after its commit the job is due; on a claimed record, what is
    /// left of that wait.
    pub delay: Duration,
}

impl OutboxRecord {
    /// The job `id` on `queue`, sealed as `sealed`, keyed by `unique_key` and
    /// due `delay` after its commit.
    pub fn new(
        queue: impl Into<String>,
        id: uuid::Uuid,
        sealed: String,
        unique_key: Option<String>,
        delay: Duration,
    ) -> Self {
        Self {
            queue: queue.into(),
            id,
            sealed,
            unique_key,
            delay,
        }
    }
}

/// The job, never its payload (`sealed`).
impl std::fmt::Debug for OutboxRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboxRecord")
            .field("queue", &self.queue)
            .field("id", &self.id)
            .field("delay", &self.delay)
            .finish_non_exhaustive()
    }
}

/// A record an [`OutboxClaim`] holds: its place in the store, how many times
/// filing it failed, and how long ago its unit wrote it.
///
/// It can grow within 7.x: build one through [`ClaimedRecord::new`].
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedRecord {
    /// The record's place in the store, which [`OutboxClaim::settle`] names it
    /// by; it grows with every record written.
    pub seq: i64,
    /// The record, its `delay` what is left of its wait.
    pub record: OutboxRecord,
    /// How many relay passes failed to file it.
    pub attempts: u32,
    /// How long ago its unit wrote it, on the store's clock.
    pub waited: Duration,
}

impl ClaimedRecord {
    /// The record at `seq`, filing it failed `attempts` times, written
    /// `waited` ago.
    pub fn new(seq: i64, record: OutboxRecord, attempts: u32, waited: Duration) -> Self {
        Self {
            seq,
            record,
            attempts,
            waited,
        }
    }
}

/// The relay's side of the outbox a data driver keeps: bound as
/// `Arc<dyn OutboxStore>` by the driver's binding, read by the queue port's
/// relay, which files what committed units wrote.
///
/// A driver whose executors keep records ([`Executor::write_outbox`](crate::Executor::write_outbox))
/// binds one, and its handles write nothing until [`arm`](Self::arm) ran: a
/// record written while no relay drains the store would never be filed.
pub trait OutboxStore: Send + Sync + 'static {
    /// Prove the store's table exists and arm the driver's executors to write
    /// into it: called once, by the relay, before the app serves. A table that
    /// is not there answers [`OutboxError::Missing`], naming the remedy, and
    /// the boot refuses.
    fn arm(&self) -> BoxFuture<'_, Result<(), OutboxError>>;

    /// Resolve once a unit that wrote records has committed, or `sweep` after
    /// the call, whichever comes first — and at once when a unit committed
    /// since the last call returned.
    fn written(&self, sweep: Duration) -> BoxFuture<'_, ()>;

    /// Lock up to `limit` records that are due and that no other relay holds,
    /// oldest first, until the claim settles; records written by a unit that
    /// has not committed are not seen.
    fn claim(&self, limit: usize) -> BoxFuture<'_, Result<Box<dyn OutboxClaim>, OutboxError>>;
}

/// The records one relay pass holds locked, settled once. Dropped unsettled,
/// it releases them unchanged, for the next pass.
pub trait OutboxClaim: Send {
    /// What the claim holds, oldest first.
    fn records(&self) -> &[ClaimedRecord];

    /// Delete the records `done` names — filed, or superseded — make each one
    /// `retry` names due again after its wait, counting the failed attempt, and
    /// release the locks, leaving every other record as it was.
    fn settle(
        self: Box<Self>,
        done: &[i64],
        retry: &[(i64, Duration)],
    ) -> BoxFuture<'static, Result<(), OutboxError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_says_its_job_and_never_its_payload() {
        let record = OutboxRecord::new(
            "notify",
            uuid::Uuid::nil(),
            r#"{"payload":"the secret"}"#.to_owned(),
            Some("key".to_owned()),
            Duration::from_secs(2),
        );
        let said = format!("{record:?}");
        assert!(said.contains("notify"), "{said}");
        assert!(!said.contains("the secret"), "{said}");
    }
}
