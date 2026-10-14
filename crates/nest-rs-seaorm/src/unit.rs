//! A unit's events name its edge (`http`, `ws`, `queue`, …) in `transport`, as
//! every other framework event does; a unit no edge dispatches is named in full.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use nest_rs_database::{
    ExecutorScope, OpenUnit, Settlement, TransactionPolicy, Unhonoured, UnitOfWork, UnitSpec,
};
use sea_orm::DatabaseConnection;

use crate::SeaOrmConfig;
use crate::error::CommitError;
use crate::executor::{Executor, FinalizeOutcome, LazyTransaction};

// What could not be done, never why the database refused: `finalize` already
// logs that, with the error.
const ESCAPED: &str =
    "the unit's transaction handle outlived it, so nothing it wrote could be committed";
const POISONED: &str =
    "a statement failed inside the unit's transaction, so nothing it wrote could be committed";
const COMMIT_FAILED: &str = "the unit's transaction could not be committed";
const UNSUPPORTED_POLICY: &str = "this driver does not support the unit's transaction policy";

/// SeaORM's unit of work: the pool for a
/// [`Pool`](TransactionPolicy::Pool) unit, and for a
/// [`PerUnit`](TransactionPolicy::PerUnit) one a transaction opened on its first
/// data-layer touch, committed when it succeeds and rolled back otherwise.
///
/// Bound as `Arc<dyn UnitOfWork>` by
/// [`SeaOrmDatabaseModule`](crate::SeaOrmDatabaseModule), so every edge opens
/// its units through it.
pub struct SeaOrmUnitOfWork {
    db: DatabaseConnection,
    /// Every pool unit installs this one handle: nothing to allocate per unit.
    pool: Arc<dyn nest_rs_database::Executor>,
    observe_conflicts: bool,
}

impl SeaOrmUnitOfWork {
    /// The unit of work over `db`, reading how to report a commit-time conflict
    /// from `config`.
    pub fn new(db: Arc<DatabaseConnection>, config: &SeaOrmConfig) -> Self {
        let db = (*db).clone();
        Self {
            pool: Arc::new(Executor::Pool(db.clone())),
            db,
            observe_conflicts: config.observe_serialization_conflicts,
        }
    }
}

impl UnitOfWork for SeaOrmUnitOfWork {
    fn open(&self, spec: &UnitSpec) -> Result<Box<dyn OpenUnit>, Unhonoured> {
        match spec.policy {
            TransactionPolicy::Pool => Ok(Box::new(OnPool(Arc::clone(&self.pool)))),
            TransactionPolicy::PerUnit => {
                let transport = spec
                    .unit
                    .edge()
                    .map_or(spec.unit.name(), nest_rs_core::Edge::as_str);
                Ok(Box::new(InTransaction {
                    lazy: Arc::new(LazyTransaction::new(self.db.clone(), transport)),
                    observe_conflicts: self.observe_conflicts
                        && spec.scope == ExecutorScope::Request,
                }))
            }
            // A policy this driver does not know is refused before the body runs.
            _ => Err(Unhonoured::deterministic(UNSUPPORTED_POLICY)),
        }
    }
}

/// A unit on the pool: each statement commits on its own, and nothing settles.
struct OnPool(Arc<dyn nest_rs_database::Executor>);

impl OpenUnit for OnPool {
    fn executor(&self) -> Arc<dyn nest_rs_database::Executor> {
        Arc::clone(&self.0)
    }

    fn settle(self: Box<Self>, _ok: bool) -> Pin<Box<dyn Future<Output = Settlement> + Send>> {
        Box::pin(Settled)
    }
}

/// A settle with nothing to do: sized zero, so boxing it allocates nothing.
struct Settled;

impl Future for Settled {
    type Output = Settlement;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Settlement> {
        Poll::Ready(Settlement::Settled)
    }
}

/// A unit in a transaction opened on its first data-layer touch.
struct InTransaction {
    lazy: Arc<LazyTransaction>,
    /// Whether a request's commit-time conflict is a `warn` of its own.
    observe_conflicts: bool,
}

impl OpenUnit for InTransaction {
    fn executor(&self) -> Arc<dyn nest_rs_database::Executor> {
        Arc::new(Executor::Lazy(Arc::clone(&self.lazy)))
    }

    fn settle(self: Box<Self>, ok: bool) -> Pin<Box<dyn Future<Output = Settlement> + Send>> {
        let Self {
            lazy,
            observe_conflicts,
        } = *self;
        let transport = lazy.transport();
        Box::pin(async move {
            match lazy.finalize(ok).await {
                FinalizeOutcome::NoTransaction
                | FinalizeOutcome::Committed
                | FinalizeOutcome::RolledBack => Settlement::Settled,
                // Deterministic: the body spawned a task holding the executor,
                // and running it again spawns the same one.
                FinalizeOutcome::Escaped => {
                    if ok {
                        Settlement::Unhonoured(Unhonoured::deterministic(ESCAPED))
                    } else {
                        Settlement::Settled
                    }
                }
                // The body swallowed a failed statement and succeeded: nothing
                // it wrote could land.
                FinalizeOutcome::Poisoned { retryable } => {
                    Settlement::Unhonoured(Unhonoured::new(POISONED, retryable))
                }
                FinalizeOutcome::CommitFailed(err) => {
                    commit_failed(&err, transport, observe_conflicts)
                }
            }
        })
    }
}

/// A commit lost mid-`COMMIT` may have landed, so only a serialization failure
/// or a deadlock is worth running again; nothing retries a request here, since
/// its body is not replayable once it ran.
fn commit_failed(
    err: &CommitError,
    transport: &'static str,
    observe_conflicts: bool,
) -> Settlement {
    let retryable = err.is_retryable_conflict();
    if observe_conflicts && retryable {
        tracing::warn!(
            target: crate::target::ORM,
            transport,
            error = %nest_rs_core::error_message(err),
            hint = "not retried here (a request's body is not replayable once it ran); \
                    use `retry::retry_on_conflict` at a programmatic transaction boundary",
            "serialization conflict at commit",
        );
    } else {
        tracing::error!(
            target: crate::target::ORM,
            transport,
            error = %nest_rs_core::error_message(err),
            retryable,
            "transaction commit failed",
        );
    }
    Settlement::Unhonoured(Unhonoured::new(COMMIT_FAILED, retryable))
}

#[cfg(test)]
mod tests {
    use nest_rs_core::operation_log::Kind;

    use super::*;

    const MESSAGE: nest_rs_core::operation_log::Unit = nest_rs_core::__private::declare_unit(
        "nest-rs-ws",
        "nest_rs::ws",
        Kind::Server,
        "ws.message",
    );

    fn unit_of_work() -> SeaOrmUnitOfWork {
        SeaOrmUnitOfWork::new(
            Arc::new(DatabaseConnection::default()),
            &SeaOrmConfig::default(),
        )
    }

    fn opened(policy: TransactionPolicy) -> Box<dyn OpenUnit> {
        unit_of_work()
            .open(&UnitSpec::new(MESSAGE, ExecutorScope::Request, policy))
            .unwrap_or_else(|why| panic!("SeaORM opens every policy it knows: {why}"))
    }

    fn seaorm(executor: &Arc<dyn nest_rs_database::Executor>) -> &Executor {
        executor
            .as_any()
            .downcast_ref::<Executor>()
            .expect("the unit installs a SeaORM executor")
    }

    #[test]
    fn a_pool_unit_runs_on_the_pool_and_a_unit_of_its_own_in_a_lazy_transaction() {
        assert!(matches!(
            seaorm(&opened(TransactionPolicy::Pool).executor()),
            Executor::Pool(_)
        ));
        let executor = opened(TransactionPolicy::PerUnit).executor();
        let Executor::Lazy(lazy) = seaorm(&executor) else {
            panic!("a unit of its own runs in a lazy transaction");
        };
        assert!(!lazy.is_opened(), "opening a unit sends nothing");
        assert_eq!(lazy.transport(), "ws", "its events name the unit's edge");
    }

    #[tokio::test]
    async fn a_unit_that_touched_nothing_settles_without_a_round_trip() {
        for policy in [TransactionPolicy::Pool, TransactionPolicy::PerUnit] {
            for ok in [true, false] {
                assert_eq!(
                    opened(policy).settle(ok).await,
                    Settlement::Settled,
                    "{policy:?}, ok = {ok}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_handle_that_outlived_a_successful_unit_fails_it() {
        let unit = opened(TransactionPolicy::PerUnit);
        let escaped = unit.executor();
        assert_eq!(
            unit.settle(true).await,
            Settlement::Unhonoured(Unhonoured::deterministic(ESCAPED))
        );
        drop(escaped);

        let unit = opened(TransactionPolicy::PerUnit);
        let escaped = unit.executor();
        assert_eq!(
            unit.settle(false).await,
            Settlement::Settled,
            "a failure stands as it is"
        );
        drop(escaped);
    }
}
