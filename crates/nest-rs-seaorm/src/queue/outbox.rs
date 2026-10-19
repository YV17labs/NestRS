//! [`SeaOrmOutbox`] — the transactional outbox over the app's Postgres: the
//! table `nestrs_outbox`, which [`OutboxMigration`](crate::queue::OutboxMigration)
//! creates, written on the caller's transaction and drained by the queue
//! port's relay.
//!
//! The store is a named escape of `Repo` (`data-layer.md`): framework
//! statements on a framework table, joining the transaction the unit already
//! holds, never a developer's row.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::future::BoxFuture;
use nest_rs_core::ContainerBuilder;
use nest_rs_database::{
    AtomicBlock, ClaimedRecord, OutboxClaim, OutboxError, OutboxRecord, OutboxStore,
};
use nest_rs_queue::OutboxModule;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, ExecResult,
    Statement, TransactionTrait, Value,
};
use tokio::sync::Notify;

use crate::atomic::AtomicExecutor;
use crate::executor::Executor;

/// The table the store keeps its records in.
pub(crate) const TABLE: &str = "nestrs_outbox";

/// What creates the table, as the boot refusing without it says.
const REMEDY: &str = "run `nestrs g migration nestrs_outbox`, then `nestrs run db migrate`";

/// The most records one `INSERT` carries: five parameters each, well inside
/// Postgres's 65535.
const ROWS_PER_INSERT: usize = 1000;

/// The relay's claim: due records no other relay holds, oldest first, each
/// with what is left of its delay and how long ago it was written, on the
/// database's clock.
const CLAIM: &str = "SELECT seq, queue, job_id, sealed, unique_key, attempts, \
     (EXTRACT(EPOCH FROM GREATEST(due_at - now(), interval '0')) * 1000)::bigint AS due_in_ms, \
     (EXTRACT(EPOCH FROM GREATEST(now() - written_at, interval '0')) * 1000)::bigint AS waited_ms \
     FROM nestrs_outbox \
     WHERE next_attempt_at <= now() \
     ORDER BY seq \
     LIMIT $1 \
     FOR UPDATE SKIP LOCKED";

/// The outbox a SeaORM unit of work writes its jobs into, and the store the
/// queue port's relay drains: bound as `Arc<dyn OutboxStore>` by
/// [`SeaOrmDatabaseModule`](crate::SeaOrmDatabaseModule) under the `queue`
/// feature.
///
/// Its executors write nothing until the relay [armed](OutboxStore::arm) it:
/// an app running no relay files every job at once, as before.
pub struct SeaOrmOutbox {
    db: DatabaseConnection,
    armed: AtomicBool,
    written: Notify,
}

impl SeaOrmOutbox {
    /// The store over `db`'s pool, unarmed.
    pub fn new(db: DatabaseConnection) -> Self {
        Self {
            db,
            armed: AtomicBool::new(false),
            written: Notify::new(),
        }
    }

    /// Whether a relay armed the store, so executors write into it.
    pub(crate) fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Acquire)
    }

    /// Wake the relay: a unit that wrote records committed.
    pub(crate) fn wake(&self) {
        self.written.notify_one();
    }

    /// Run a removal of a kept record on the store's pool, outside every
    /// transaction, answering whether it deleted one.
    pub(crate) async fn remove_on_pool(&self, removal: Statement) -> Result<bool, OutboxError> {
        removed(self.db.execute_raw(removal).await)
    }
}

/// The pool a boundary keeping an outbox hands on — to its after-commit work,
/// to a step out of its transaction: a push there is filed at once, as on any
/// pool, while a block of its own keeps its pushes until it commits and a
/// cancel deletes a record no relay has filed, as on a pool unit.
pub(crate) struct KeepingPool {
    pool: Executor,
    outbox: Arc<SeaOrmOutbox>,
}

impl KeepingPool {
    /// `pool`, keeping `outbox`.
    pub(crate) fn new(pool: DatabaseConnection, outbox: Arc<SeaOrmOutbox>) -> Self {
        Self {
            pool: Executor::Pool(pool),
            outbox,
        }
    }

    /// The outbox a cancel deletes a kept record from, once a relay armed it.
    fn outbox(&self) -> Option<&Arc<SeaOrmOutbox>> {
        Some(&self.outbox).filter(|outbox| outbox.is_armed())
    }
}

impl nest_rs_database::Executor for KeepingPool {
    fn as_any(&self) -> &dyn Any {
        &self.pool
    }

    fn atomic(&self) -> Option<Arc<dyn AtomicBlock>> {
        let own = self
            .pool
            .own_block()?
            .keeping(Some(Arc::clone(&self.outbox)));
        Some(Arc::new(AtomicExecutor::own(Arc::new(own))))
    }

    fn remove_outbox<'a>(
        &'a self,
        queue: &'a str,
        id: uuid::Uuid,
    ) -> Option<BoxFuture<'a, Result<bool, OutboxError>>> {
        let outbox = self.outbox()?;
        Some(Box::pin(outbox.remove_on_pool(removal_of_job(queue, id))))
    }

    fn remove_outbox_unique<'a>(
        &'a self,
        queue: &'a str,
        key: &'a str,
    ) -> Option<BoxFuture<'a, Result<bool, OutboxError>>> {
        let outbox = self.outbox()?;
        Some(Box::pin(outbox.remove_on_pool(removal_of_key(queue, key))))
    }
}

impl OutboxStore for SeaOrmOutbox {
    fn arm(&self) -> BoxFuture<'_, Result<(), OutboxError>> {
        Box::pin(async move {
            let present = self
                .db
                .query_one_raw(Statement::from_string(
                    DbBackend::Postgres,
                    format!("SELECT to_regclass('{TABLE}') IS NOT NULL AS present"),
                ))
                .await
                .and_then(|row| match row {
                    Some(row) => row.try_get::<bool>("", "present"),
                    None => Ok(false),
                })
                .map_err(database)?;
            if !present {
                return Err(OutboxError::Missing {
                    table: TABLE,
                    remedy: REMEDY,
                });
            }
            self.armed.store(true, Ordering::Release);
            Ok(())
        })
    }

    fn written(&self, sweep: Duration) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            drop(tokio::time::timeout(sweep, self.written.notified()).await);
        })
    }

    fn claim(&self, limit: usize) -> BoxFuture<'_, Result<Box<dyn OutboxClaim>, OutboxError>> {
        Box::pin(async move {
            let txn = self.db.begin().await.map_err(database)?;
            let limit = i64::try_from(limit).unwrap_or(i64::MAX);
            let rows = txn
                .query_all_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    CLAIM,
                    [Value::from(limit)],
                ))
                .await
                .map_err(database)?;
            let records = rows
                .iter()
                .map(|row| {
                    let millis = |column| {
                        row.try_get::<i64>("", column)
                            .map(|ms| Duration::from_millis(u64::try_from(ms).unwrap_or(0)))
                    };
                    let record = OutboxRecord::new(
                        row.try_get::<String>("", "queue")?,
                        row.try_get::<uuid::Uuid>("", "job_id")?,
                        row.try_get::<String>("", "sealed")?,
                        row.try_get::<Option<String>>("", "unique_key")?,
                        millis("due_in_ms")?,
                    );
                    Ok(ClaimedRecord::new(
                        row.try_get::<i64>("", "seq")?,
                        record,
                        u32::try_from(row.try_get::<i32>("", "attempts")?).unwrap_or(0),
                        millis("waited_ms")?,
                    ))
                })
                .collect::<Result<Vec<_>, DbErr>>()
                .map_err(database)?;
            Ok(Box::new(SeaOrmClaim { txn, records }) as Box<dyn OutboxClaim>)
        })
    }
}

/// A claim's records, locked by the transaction it holds; dropped unsettled,
/// that transaction rolls back and releases them unchanged.
struct SeaOrmClaim {
    txn: DatabaseTransaction,
    records: Vec<ClaimedRecord>,
}

impl OutboxClaim for SeaOrmClaim {
    fn records(&self) -> &[ClaimedRecord] {
        &self.records
    }

    fn settle(
        self: Box<Self>,
        done: &[i64],
        retry: &[(i64, Duration)],
    ) -> BoxFuture<'static, Result<(), OutboxError>> {
        // The places are the claim's own integers, never a caller's text.
        let mut statements = Vec::new();
        if !done.is_empty() {
            statements.push(format!(
                "DELETE FROM {TABLE} WHERE seq IN ({})",
                done.iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !retry.is_empty() {
            statements.push(format!(
                "UPDATE {TABLE} SET attempts = attempts + 1, \
                 next_attempt_at = clock_timestamp() + w.wait_ms * interval '1 millisecond' \
                 FROM (VALUES {}) AS w(seq, wait_ms) WHERE {TABLE}.seq = w.seq",
                retry
                    .iter()
                    .map(|(seq, wait)| format!(
                        "({seq}::bigint, {}::bigint)",
                        i64::try_from(wait.as_millis()).unwrap_or(i64::MAX)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let SeaOrmClaim { txn, .. } = *self;
        Box::pin(async move {
            for statement in statements {
                txn.execute_unprepared(&statement).await.map_err(database)?;
            }
            txn.commit().await.map_err(database)
        })
    }
}

/// A driver's error as the outbox's, said through its `SafeLink`.
fn database(err: DbErr) -> OutboxError {
    OutboxError::Database(Box::new(err))
}

/// Write `records` with the statement runner `run` — the unit's transaction,
/// in its turn — in as few `INSERT`s as Postgres takes.
pub(crate) async fn insert<F, Fut, T>(
    records: Vec<OutboxRecord>,
    mut run: F,
) -> Result<(), OutboxError>
where
    F: FnMut(Statement) -> Fut,
    Fut: std::future::Future<Output = Result<T, DbErr>>,
{
    let mut records = records.into_iter();
    while !records.as_slice().is_empty() {
        let rows = records.len().min(ROWS_PER_INSERT);
        let mut sql =
            format!("INSERT INTO {TABLE} (queue, job_id, sealed, unique_key, due_at) VALUES ");
        let mut values = Vec::with_capacity(rows * 5);
        for (row, record) in records.by_ref().take(rows).enumerate() {
            let at = row * 5;
            if row > 0 {
                sql.push_str(", ");
            }
            // The statement's own clock, the nearest to the commit it can read.
            sql.push_str(&format!(
                "(${}, ${}, ${}, ${}, clock_timestamp() + ${}::bigint * interval '1 millisecond')",
                at + 1,
                at + 2,
                at + 3,
                at + 4,
                at + 5,
            ));
            values.extend([
                Value::from(record.queue),
                Value::from(record.id),
                Value::from(record.sealed),
                Value::from(record.unique_key),
                Value::from(i64::try_from(record.delay.as_millis()).unwrap_or(i64::MAX)),
            ]);
        }
        run(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            values,
        ))
        .await
        .map_err(database)?;
    }
    Ok(())
}

/// The statement deleting the record of job `id` on `queue` no relay has
/// filed.
///
/// No `SKIP LOCKED`: a record a relay's claim holds is waited for, then
/// deleted if the relay left it or found gone once the backend holds the job,
/// so a cancel never answers `false` for a job filed a moment later. The wait
/// cannot deadlock, since a claim never waits on a unit's rows.
pub(crate) fn removal_of_job(queue: &str, id: uuid::Uuid) -> Statement {
    Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("DELETE FROM {TABLE} WHERE queue = $1 AND job_id = $2"),
        [Value::from(queue.to_owned()), Value::from(id)],
    )
}

/// The statement deleting the records carrying `key` on `queue` no relay has
/// filed, waiting for a claim as [`removal_of_job`] does.
pub(crate) fn removal_of_key(queue: &str, key: &str) -> Statement {
    Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("DELETE FROM {TABLE} WHERE queue = $1 AND unique_key = $2"),
        [Value::from(queue.to_owned()), Value::from(key.to_owned())],
    )
}

/// Whether a removal deleted a record.
pub(crate) fn removed(result: Result<ExecResult, DbErr>) -> Result<bool, OutboxError> {
    result
        .map(|done| done.rows_affected() > 0)
        .map_err(database)
}

/// Bind the store for the binding's `collect`: the store over the pool, as
/// itself and as `dyn OutboxStore`, and the queue port's relay that drains it.
pub(crate) fn collect(builder: ContainerBuilder) -> ContainerBuilder {
    builder
        .provide_factory_after::<SeaOrmOutbox, (DatabaseConnection,), _, _>(
            |container| async move {
                let db = container.get::<DatabaseConnection>().ok_or_else(|| {
                    anyhow::anyhow!("SeaOrmDatabaseModule: no pool to keep the outbox in")
                })?;
                Ok(SeaOrmOutbox::new((*db).clone()))
            },
        )
        .provide_factory_after::<Arc<dyn OutboxStore>, (SeaOrmOutbox,), _, _>(
            |container| async move {
                let store = container.get::<SeaOrmOutbox>().ok_or_else(|| {
                    anyhow::anyhow!("SeaOrmDatabaseModule: the outbox store was not built")
                })?;
                Ok(store as Arc<dyn OutboxStore>)
            },
        )
        .import::<OutboxModule>()
}

/// The binding's `register`: the relay's module, imported in both phases.
pub(crate) fn register(builder: ContainerBuilder) -> ContainerBuilder {
    builder.import::<OutboxModule>()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn record(n: u8, key: Option<&str>) -> OutboxRecord {
        OutboxRecord::new(
            "notify",
            uuid::Uuid::from_bytes([n; 16]),
            format!(r#"{{"payload":{n}}}"#),
            key.map(str::to_owned),
            Duration::from_millis(1500),
        )
    }

    #[tokio::test]
    async fn records_are_written_in_one_insert_each_row_on_its_own_parameters() {
        let mut sent = Vec::new();
        insert(vec![record(1, None), record(2, Some("k"))], |statement| {
            sent.push(statement);
            std::future::ready(Ok::<_, DbErr>(()))
        })
        .await
        .expect("written");
        let [statement] = sent.as_slice() else {
            panic!("one statement: {sent:?}");
        };
        assert!(
            statement.sql.ends_with(
                "($6, $7, $8, $9, clock_timestamp() + $10::bigint * interval '1 millisecond')"
            ),
            "{}",
            statement.sql
        );
        let values = statement.values.as_ref().expect("parameters").0.as_slice();
        assert_eq!(values.len(), 10);
        assert_eq!(
            values[4],
            Value::from(1500_i64),
            "the delay, in milliseconds"
        );
        assert_eq!(values[8], Value::from(Some("k".to_owned())));
    }

    #[tokio::test]
    async fn a_write_past_what_one_statement_carries_is_split_on_the_same_transaction() {
        let mut sent = 0;
        let records = (0..=ROWS_PER_INSERT).map(|n| record((n % 200) as u8, None));
        insert(records.collect(), |_| {
            sent += 1;
            std::future::ready(Ok::<_, DbErr>(()))
        })
        .await
        .expect("written");
        assert_eq!(sent, 2);
    }

    #[test]
    fn a_store_is_unarmed_until_a_relay_arms_it() {
        let store = SeaOrmOutbox::new(DatabaseConnection::default());
        assert!(!store.is_armed());
    }

    /// A store a relay armed, without the round trip arming takes.
    pub(crate) fn armed() -> Arc<SeaOrmOutbox> {
        let store = SeaOrmOutbox::new(DatabaseConnection::default());
        store.armed.store(true, Ordering::Release);
        Arc::new(store)
    }
}
