use std::time::Duration;

use futures_util::future::BoxFuture;

use crate::error::OutboxError;

/// One job a unit's transaction keeps until it commits: written by the queue
/// port into the outbox of the transaction the ambient executor holds
/// ([`Outbox::write`]), read back by its relay through an [`OutboxStore`], and
/// opaque to this port, which names no queue type.
///
/// It can grow in a minor release: build one through [`OutboxRecord::new`].
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
/// It can grow in a minor release: build one through [`ClaimedRecord::new`].
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

nest_rs_core::inventory::submit! {
    nest_rs_core::DependencyRemedy::new(
        std::any::TypeId::of::<std::sync::Arc<dyn OutboxStore>>,
        "Import a data driver's binding: `nest_rs::seaorm::SeaOrmDatabaseModule` binds the \
         outbox store the queue port's relay drains.",
    )
}

/// What an outbox keeps and no relay has filed yet, as a cancel reaches it
/// from the ambient executor: on the pool
/// ([`HeldPool::outbox`](crate::HeldPool::outbox)), the records committed
/// units wrote; in a transaction, through its [`Outbox`], those and its own.
pub trait OutboxKept: Send + Sync {
    /// Delete the record of job `id` on `queue` that no relay has filed yet,
    /// on the handle's own unit, answering whether there was one. A record a
    /// relay is filing is waited for: the removal answers once that relay
    /// settled, deleting the record it left, or finding none once the backend
    /// holds the job.
    fn remove<'a>(
        &'a self,
        queue: &'a str,
        id: uuid::Uuid,
    ) -> BoxFuture<'a, Result<bool, OutboxError>>;

    /// [`remove`](Self::remove) for the record carrying the unique key `key`
    /// on `queue`.
    fn remove_unique<'a>(
        &'a self,
        queue: &'a str,
        key: &'a str,
    ) -> BoxFuture<'a, Result<bool, OutboxError>>;
}

/// The outbox as a transaction writes into it
/// ([`HeldTransaction::outbox`](crate::HeldTransaction::outbox)): the queue
/// port's side of the store its relay drains.
pub trait Outbox: OutboxKept {
    /// Write `records` on the handle's transaction, so they commit or roll
    /// back with its writes. What the driver owes: the records land in the
    /// store exactly when the unit commits, the relay's
    /// [`written`](OutboxStore::written) wakes once it did, and a unit that
    /// never opened its transaction opens it, since the write is a touch. A
    /// failed write fails the unit as a failed statement does.
    fn write(&self, records: Vec<OutboxRecord>) -> BoxFuture<'_, Result<(), OutboxError>>;
}

/// The relay's side of the outbox a data driver keeps: bound as
/// `Arc<dyn OutboxStore>` by the driver's binding, read by the queue port's
/// relay, which files what committed units wrote.
///
/// A driver whose transactions keep records ([`Outbox::write`]) binds one, and
/// its handles answer no outbox until [`arm`](Self::arm) ran: a record written
/// while no relay drains the store would never be filed.
pub trait OutboxStore: Send + Sync + 'static {
    /// Prove the store's table exists and arm the driver's handles to write
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
