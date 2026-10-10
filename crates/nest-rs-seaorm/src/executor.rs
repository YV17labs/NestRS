//! An enum rather than a trait object: `ConnectionTrait` has generic methods,
//! so it is not object-safe.

use std::any::Any;
use std::future::Future;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use nest_rs_database::Deferred;

use crate::error::CommitError;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, ExecResult,
    QueryResult, Statement, TransactionTrait,
};

pub use nest_rs_database::ExecutorScope;
pub use nest_rs_database::current_executor_scope;

/// The connection a request's queries run against: the shared pool, the
/// per-request [`DatabaseTransaction`], or a transaction opened lazily on
/// first use. Cheap to clone.
#[derive(Clone)]
pub enum Executor {
    /// The shared connection pool — safe (read) methods and system/job work run
    /// here, outside any transaction.
    Pool(DatabaseConnection),
    /// A transaction the **caller** opened and installed itself, through
    /// `with_executor`; the framework never builds one.
    ///
    /// Its commit is the caller's, so work handed to
    /// [`after_commit`](nest_rs_database::after_commit) inside it runs at once:
    /// the caller emits after its own `commit`.
    Txn(Arc<DatabaseTransaction>),
    /// A transaction opened on **first data-layer touch**. Installed by the
    /// HTTP `DbContext` for mutating methods, so a request a guard denies (or
    /// one that never queries) costs no `BEGIN`/`ROLLBACK` round-trip at all.
    Lazy(Arc<LazyTransaction>),
}

/// The deferred-`BEGIN` state behind [`Executor::Lazy`]: the pool to open on,
/// and the transaction once something touched the data layer. Concurrent
/// first touches race through a [`tokio::sync::OnceCell`], so exactly one
/// transaction opens per request.
pub struct LazyTransaction {
    pool: DatabaseConnection,
    /// The edge this boundary serves, named by every event it emits.
    transport: &'static str,
    cell: tokio::sync::OnceCell<Arc<DatabaseTransaction>>,
    /// Set the instant [`finalize`](Self::finalize) begins, so [`Drop`] can tell
    /// a settled boundary from an **abandoned** one.
    settled: std::sync::atomic::AtomicBool,
    /// Set the moment a statement on **this** transaction returns an error, to
    /// [`POISON_TRANSIENT`] or [`POISON_DETERMINISTIC`] according to that error.
    ///
    /// Postgres aborts the transaction on the first failed statement, and a
    /// `COMMIT` on an aborted transaction *succeeds* while rolling back: without
    /// this, a boundary swallowing a `DbErr` would report writes that never
    /// landed. A nested transaction's statements run on their own handle and
    /// never reach here; opening one does.
    poisoned: AtomicU8,
    /// The work [`after_commit`](nest_rs_database::after_commit) handed this
    /// boundary: run by [`finalize`](Self::finalize) once the transaction has
    /// committed, dropped unrun when it has not. `None` once settling begins, so
    /// an escaped handle's late registration is refused.
    after_commit: Mutex<Option<Vec<Deferred>>>,
}

/// No statement on this transaction has failed.
const POISON_CLEAN: u8 = 0;
/// One failed on something a re-run would hit again — a constraint violation, a
/// type error, a missing relation.
const POISON_DETERMINISTIC: u8 = 1;
/// One failed on something a fresh attempt could clear
/// ([`is_transient_failure`]) — a serialization conflict, a deadlock, or the
/// connection going away before anything could be committed.
///
/// [`is_transient_failure`]: crate::retry::is_transient_failure
const POISON_TRANSIENT: u8 = 2;

impl LazyTransaction {
    /// A lazy transaction over `pool` — nothing is opened yet. `transport` names
    /// the edge for every event this boundary emits.
    pub fn new(pool: DatabaseConnection, transport: &'static str) -> Self {
        Self {
            pool,
            transport,
            cell: tokio::sync::OnceCell::new(),
            settled: std::sync::atomic::AtomicBool::new(false),
            poisoned: AtomicU8::new(POISON_CLEAN),
            after_commit: Mutex::new(Some(Vec::new())),
        }
    }

    /// Keep `work` for after the commit, to run on the **pool** under the
    /// boundary's scope and the caller's ability.
    ///
    /// Captures the pool, never this `LazyTransaction`: a held `Arc` to the
    /// boundary would make every boundary that held work read as escaped.
    fn hold(&self, work: Deferred) {
        let scope = nest_rs_database::current_executor_scope();
        let ability = nest_rs_authz::current_ability();
        let pool = Executor::Pool(self.pool.clone());
        let work: Deferred = Box::pin(async move {
            let work: Deferred = match ability {
                Some(ability) => Box::pin(nest_rs_authz::with_ability(ability, work)),
                None => work,
            };
            match scope {
                Some(ExecutorScope::Request) => with_request_executor(pool, work).await,
                Some(ExecutorScope::Job) => with_job_executor(pool, work).await,
                None => with_executor(pool, work).await,
            }
        });
        let refused = {
            let mut held = self
                .after_commit
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match held.as_mut() {
                Some(held) => {
                    held.push(work);
                    None
                }
                None => Some(work),
            }
        };
        if let Some(work) = refused {
            drop(work);
            tracing::warn!(
                target: crate::TARGET,
                transport = self.transport,
                outcome = "discarded",
                "after-commit work registered on a boundary that has already settled; \
                 it will not run",
            );
        }
    }

    /// The held work, taken for settling — after which nothing more is held.
    fn take_after_commit(&self) -> Vec<Deferred> {
        self.after_commit
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .unwrap_or_default()
    }

    /// Run one statement on the request's transaction, poisoning the boundary
    /// on **either** failure: a `?` on `txn_ref` would let a failed `BEGIN`
    /// escape the flag.
    async fn run<'a, T, F, Fut>(&'a self, statement: F) -> Result<T, DbErr>
    where
        F: FnOnce(&'a DatabaseTransaction) -> Fut,
        Fut: Future<Output = Result<T, DbErr>> + 'a,
    {
        self.poison(match self.txn_ref().await {
            Ok(txn) => statement(txn).await,
            Err(err) => Err(err),
        })
    }

    /// The request's transaction, opening it on the first call.
    async fn txn_ref(&self) -> Result<&DatabaseTransaction, DbErr> {
        Ok(self
            .cell
            .get_or_try_init(|| async { self.pool.begin().await.map(Arc::new) })
            .await?
            .as_ref())
    }

    /// The transaction, if a data-layer touch opened one — consumed by
    /// [`finalize`](LazyTransaction::finalize). `None` means no `BEGIN` was
    /// ever issued.
    pub fn into_opened(mut self) -> Option<Arc<DatabaseTransaction>> {
        self.cell.take()
    }

    /// Whether a transaction has been opened.
    pub fn is_opened(&self) -> bool {
        self.cell.get().is_some()
    }

    /// Record that a statement on this transaction failed, and what the database
    /// said about it. **The first failure wins**: everything after it fails
    /// with `25P02`, which says nothing about why.
    fn poison<T>(&self, result: Result<T, DbErr>) -> Result<T, DbErr> {
        if let Err(err) = &result {
            // A statement, not a `COMMIT`: nothing is durable yet, so the broad
            // predicate is safe here.
            let verdict = if crate::retry::is_transient_failure(err) {
                POISON_TRANSIENT
            } else {
                POISON_DETERMINISTIC
            };
            #[expect(
                clippy::let_underscore_must_use,
                reason = "the first failure's verdict stands; a later one finds the slot taken"
            )]
            let _ = self.poisoned.compare_exchange(
                POISON_CLEAN,
                verdict,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
        result
    }

    /// Force the request transaction open (if a data-layer touch has not
    /// already) and start a **SAVEPOINT** on it, through [`run`](Self::run) so
    /// a refused `SAVEPOINT` poisons the boundary.
    pub(crate) async fn begin_nested(&self) -> Result<DatabaseTransaction, DbErr> {
        self.run(|txn| txn.begin()).await
    }

    /// Settle the boundary's lazily opened transaction: commit on `success`,
    /// roll back otherwise, do nothing when no data-layer touch ever opened
    /// one — then run the work held for after the commit, or drop it. It runs
    /// on [`Committed`](FinalizeOutcome::Committed), and on
    /// [`NoTransaction`](FinalizeOutcome::NoTransaction) when the boundary
    /// succeeded; every other outcome drops it.
    ///
    /// An executor clone lingering in a spawned task cannot be committed:
    /// [`FinalizeOutcome::Escaped`], which the caller must fail loudly on
    /// success. A commit failure is returned unlogged, for the transport to
    /// classify.
    pub async fn finalize(self: Arc<Self>, success: bool) -> FinalizeOutcome {
        let transport = self.transport;
        // Settling consumes `self` before `COMMIT` is awaited, so its `Drop`
        // cannot report a future dropped mid-`COMMIT`: this guard does.
        let mut abandoned = AbandonedDuringSettle {
            transport,
            armed: self.is_opened(),
        };
        self.settled.store(true, Ordering::Relaxed);
        let after_commit = self.take_after_commit();
        let outcome = Self::settle(self, success).await;
        abandoned.armed = false;
        let committed = match outcome {
            FinalizeOutcome::Committed => true,
            FinalizeOutcome::NoTransaction => success,
            _ => false,
        };
        settle_after_commit(transport, after_commit, committed).await;
        outcome
    }

    async fn settle(self: Arc<Self>, success: bool) -> FinalizeOutcome {
        let transport = self.transport;
        let escaped_outcome = if success {
            "rollback_and_fail"
        } else {
            "rollback"
        };
        let lazy = match Arc::try_unwrap(self) {
            Ok(lazy) => lazy,
            Err(escaped) => {
                let opened = escaped.is_opened();
                drop(escaped);
                tracing::error!(
                    target: crate::TARGET,
                    transport,
                    opened,
                    outcome = escaped_outcome,
                    "executor escaped into a spawned task"
                );
                return FinalizeOutcome::Escaped;
            }
        };
        let flag = lazy.poisoned.load(Ordering::Relaxed);
        let poisoned = (success && flag != POISON_CLEAN).then_some(flag == POISON_TRANSIENT);
        let Some(txn) = lazy.into_opened() else {
            // A swallowed failed `BEGIN` is not "nothing to settle".
            if let Some(retryable) = poisoned {
                tracing::error!(
                    target: crate::TARGET,
                    transport,
                    outcome = "fail",
                    retryable,
                    "a statement failed before this boundary could open its transaction, \
                     but the boundary reported success; nothing it meant to write was written"
                );
                return FinalizeOutcome::Poisoned { retryable };
            }
            return FinalizeOutcome::NoTransaction;
        };
        let txn = match Arc::try_unwrap(txn) {
            Ok(txn) => txn,
            Err(escaped) => {
                drop(escaped);
                tracing::error!(
                    target: crate::TARGET,
                    transport,
                    opened = true,
                    outcome = escaped_outcome,
                    "transaction escaped into a spawned task"
                );
                return FinalizeOutcome::Escaped;
            }
        };
        // The transaction is aborted: its `COMMIT` would succeed having written
        // nothing.
        if let Some(retryable) = poisoned {
            if let Err(err) = txn.rollback().await {
                tracing::error!(
                    target: crate::TARGET,
                    transport,
                    error = %nest_rs_core::error_message(&err),
                    "poisoned transaction rollback failed"
                );
            }
            tracing::error!(
                target: crate::TARGET,
                transport,
                outcome = "rollback_and_fail",
                retryable,
                "a statement failed inside this transaction but the boundary reported success; \
                 nothing it wrote could be committed"
            );
            return FinalizeOutcome::Poisoned { retryable };
        }
        if success {
            match txn.commit().await {
                Ok(()) => FinalizeOutcome::Committed,
                Err(err) => FinalizeOutcome::CommitFailed(CommitError(err)),
            }
        } else {
            if let Err(err) = txn.rollback().await {
                tracing::error!(
                    target: crate::TARGET,
                    transport,
                    error = %nest_rs_core::error_message(&err),
                    "transaction rollback failed"
                );
            }
            FinalizeOutcome::RolledBack
        }
    }
}

impl Drop for LazyTransaction {
    /// The boundary was **abandoned** before settling (a worker's drain window
    /// closing on it). sea-orm cannot roll back while the connection is busy,
    /// so the row locks stay held until the statement drains server-side: the
    /// event is what the framework owes.
    fn drop(&mut self) {
        let held = self
            .after_commit
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or(0, Vec::len);
        report_discarded(self.transport, held);
        if self.settled.load(Ordering::Relaxed) || self.cell.get().is_none() {
            return;
        }
        report_abandoned(self.transport);
    }
}

/// Run the boundary's after-commit work when it `committed`, or drop it. A
/// panic is contained: unwinding would turn a committed attempt into a failed
/// one a queue replays.
async fn settle_after_commit(transport: &'static str, held: Vec<Deferred>, committed: bool) {
    if !committed {
        report_discarded(transport, held.len());
        return;
    }
    for work in held {
        if let Err(payload) = nest_rs_core::panic::contain(work).await {
            nest_rs_core::contained_panic!(
                target: crate::TARGET,
                payload.as_ref(),
                "after-commit work panicked; the transaction had already committed",
                transport,
            );
        }
    }
}

/// `debug`: the expected consequence of a rollback the edge already reports.
fn report_discarded(transport: &'static str, discarded: usize) {
    if discarded > 0 {
        tracing::debug!(
            target: crate::TARGET,
            transport,
            discarded,
            "after-commit work discarded: the boundary committed nothing",
        );
    }
}

/// Reports a boundary whose *settling* was abandoned — the future awaiting
/// `COMMIT` or `ROLLBACK` was dropped before it returned.
struct AbandonedDuringSettle {
    transport: &'static str,
    armed: bool,
}

impl Drop for AbandonedDuringSettle {
    fn drop(&mut self) {
        if self.armed {
            report_abandoned(self.transport);
        }
    }
}

fn report_abandoned(transport: &'static str) {
    tracing::warn!(
        target: crate::TARGET,
        transport,
        outcome = "abandoned",
        "transaction abandoned without settling; its locks are held until the \
         abandoned statement drains",
    );
}

/// How [`LazyTransaction::finalize`] settled the boundary's transaction.
#[derive(Debug)]
pub enum FinalizeOutcome {
    /// Nothing ever touched the data layer — no `BEGIN` was issued.
    NoTransaction,
    /// The transaction committed.
    Committed,
    /// The transaction rolled back (a rollback error was already logged).
    RolledBack,
    /// A handle escaped into a task outliving the boundary; nothing could be
    /// committed (the leaked handle's eventual `Drop` rolls back). On a
    /// success path the caller must fail the response loudly.
    ///
    /// No "nothing opened" flag: the escaped handle is still live, and can open
    /// a transaction and write later.
    Escaped,
    /// The commit itself failed — the caller classifies and logs it.
    CommitFailed(CommitError),
    /// A statement failed inside the transaction, yet the boundary reported
    /// success: nothing was committed. The caller fails the outcome loudly, like
    /// [`CommitFailed`](Self::CommitFailed). Already logged at `error`.
    Poisoned {
        /// Whether the failed statement failed on something a fresh attempt
        /// could clear, read from the database's error.
        ///
        /// It answers for this transaction only: work done outside it (an HTTP
        /// call, an S3 write,
        /// [`non_transactional`](nest_rs_database::Executor::non_transactional))
        /// is replayed with the body.
        retryable: bool,
    },
}

#[async_trait]
impl ConnectionTrait for Executor {
    fn get_database_backend(&self) -> DbBackend {
        match self {
            Executor::Pool(c) => c.get_database_backend(),
            Executor::Txn(t) => t.get_database_backend(),
            Executor::Lazy(l) => l.pool.get_database_backend(),
        }
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        match self {
            Executor::Pool(c) => c.execute_raw(stmt).await,
            Executor::Txn(t) => t.execute_raw(stmt).await,
            Executor::Lazy(l) => l.run(|txn| txn.execute_raw(stmt)).await,
        }
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        match self {
            Executor::Pool(c) => c.execute_unprepared(sql).await,
            Executor::Txn(t) => t.execute_unprepared(sql).await,
            Executor::Lazy(l) => l.run(|txn| txn.execute_unprepared(sql)).await,
        }
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        match self {
            Executor::Pool(c) => c.query_one_raw(stmt).await,
            Executor::Txn(t) => t.query_one_raw(stmt).await,
            Executor::Lazy(l) => l.run(|txn| txn.query_one_raw(stmt)).await,
        }
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        match self {
            Executor::Pool(c) => c.query_all_raw(stmt).await,
            Executor::Txn(t) => t.query_all_raw(stmt).await,
            Executor::Lazy(l) => l.run(|txn| txn.query_all_raw(stmt)).await,
        }
    }

    fn support_returning(&self) -> bool {
        match self {
            Executor::Pool(c) => c.support_returning(),
            Executor::Txn(t) => t.support_returning(),
            Executor::Lazy(l) => l.pool.support_returning(),
        }
    }

    fn is_mock_connection(&self) -> bool {
        match self {
            Executor::Pool(c) => c.is_mock_connection(),
            Executor::Txn(t) => t.is_mock_connection(),
            Executor::Lazy(l) => l.pool.is_mock_connection(),
        }
    }
}

impl nest_rs_database::Executor for Executor {
    fn as_any(&self) -> &dyn Any {
        self
    }

    /// Only a lazy transaction **nothing has opened yet** yields a pool handle:
    /// once `BEGIN` went out, stepping out would split the request's view of
    /// the data.
    fn non_transactional(&self) -> Option<Arc<dyn nest_rs_database::Executor>> {
        match self {
            Executor::Lazy(lazy) if !lazy.is_opened() => {
                Some(Arc::new(Executor::Pool(lazy.pool.clone())))
            }
            _ => None,
        }
    }

    /// A lazy transaction holds the work for its boundary to settle; a pool and
    /// a caller's [`Txn`](Executor::Txn) hand it back to run at once.
    fn after_commit(&self, work: Deferred) -> Option<Deferred> {
        match self {
            Executor::Lazy(lazy) => {
                lazy.hold(work);
                None
            }
            Executor::Pool(_) | Executor::Txn(_) => Some(work),
        }
    }
}

/// The SeaORM `Executor` installed in the ambient task-local for this
/// request or job, or `None` outside any scope. A downcast miss (another ORM's
/// handle) logs an error and surfaces as `None`.
pub fn current_executor() -> Option<Executor> {
    let dynamic = nest_rs_database::current_executor()?;
    match dynamic.as_any().downcast_ref::<Executor>() {
        Some(executor) => Some(executor.clone()),
        None => {
            tracing::error!(
                target: crate::TARGET,
                reason = "executor_downcast_miss",
                "ambient executor is not a SeaORM Executor"
            );
            None
        }
    }
}

/// Install `executor` without tagging a scope: with no ambient ability `Repo`
/// denies every row, as on a request; only [`with_job_executor`] is unscoped.
pub async fn with_executor<F: Future>(executor: Executor, fut: F) -> F::Output {
    nest_rs_database::with_executor(Arc::new(executor), fut).await
}

/// Install `executor` tagged as a **request** scope, so `Repo` applies the
/// ambient ability's `WHERE` (fail-closed with no ability, like any request).
pub async fn with_request_executor<F: Future>(executor: Executor, fut: F) -> F::Output {
    nest_rs_database::with_request_executor(Arc::new(executor), fut).await
}

/// Install `executor` tagged as a **job** scope — the one unscoped path, for
/// system work (cron, queue) that has no principal to scope to.
pub async fn with_job_executor<F: Future>(executor: Executor, fut: F) -> F::Output {
    nest_rs_database::with_job_executor(Arc::new(executor), fut).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> Executor {
        Executor::Pool(DatabaseConnection::default())
    }

    fn as_seaorm(executor: &Arc<dyn nest_rs_database::Executor>) -> &Executor {
        executor
            .as_any()
            .downcast_ref::<Executor>()
            .expect("the handle is a SeaORM Executor")
    }

    #[test]
    fn an_unopened_lazy_transaction_hands_out_its_pool() {
        let lazy = Executor::Lazy(Arc::new(LazyTransaction::new(
            DatabaseConnection::default(),
            "test",
        )));
        let handle = nest_rs_database::Executor::non_transactional(&lazy)
            .expect("an unopened lazy transaction can step out");
        assert!(matches!(as_seaorm(&handle), Executor::Pool(_)));
    }

    #[test]
    fn a_pool_has_nothing_to_step_out_of() {
        assert!(nest_rs_database::Executor::non_transactional(&pool()).is_none());
    }

    #[tokio::test]
    async fn no_ambient_executor_outside_a_scope() {
        assert!(current_executor().is_none());
        assert!(current_executor_scope().is_none());
    }

    #[tokio::test]
    async fn with_executor_installs_the_value_but_no_scope() {
        with_executor(pool(), async {
            assert!(matches!(current_executor(), Some(Executor::Pool(_))));
            assert!(current_executor_scope().is_none());
        })
        .await;
    }

    #[tokio::test]
    async fn with_request_executor_tags_the_scope_as_request() {
        with_request_executor(pool(), async {
            assert_eq!(current_executor_scope(), Some(ExecutorScope::Request));
        })
        .await;
    }

    #[tokio::test]
    async fn with_job_executor_tags_the_scope_as_job() {
        with_job_executor(pool(), async {
            assert_eq!(current_executor_scope(), Some(ExecutorScope::Job));
        })
        .await;
    }

    #[tokio::test]
    async fn executor_unwinds_when_the_scope_ends() {
        with_request_executor(pool(), async {
            assert!(current_executor().is_some());
        })
        .await;
        assert!(
            current_executor().is_none(),
            "the task-local must unwind once the scope future resolves",
        );
        assert!(current_executor_scope().is_none());
    }

    #[tokio::test]
    async fn with_executor_leaves_scope_task_local_untouched() {
        assert!(current_executor_scope().is_none());
        with_executor(pool(), async {
            assert!(current_executor().is_some());
            assert!(current_executor_scope().is_none());
        })
        .await;
        assert!(current_executor().is_none());
        assert!(current_executor_scope().is_none());
    }

    #[tokio::test]
    async fn current_executor_returns_a_fresh_clone_per_call() {
        with_request_executor(pool(), async {
            let a = current_executor().expect("installed");
            let b = current_executor().expect("still installed");
            assert!(matches!(a, Executor::Pool(_)));
            assert!(matches!(b, Executor::Pool(_)));
        })
        .await;
    }

    #[tokio::test]
    async fn nested_scope_shadows_then_restores_the_outer_scope() {
        with_request_executor(pool(), async {
            assert_eq!(current_executor_scope(), Some(ExecutorScope::Request));
            with_job_executor(pool(), async {
                assert_eq!(
                    current_executor_scope(),
                    Some(ExecutorScope::Job),
                    "the inner scope wins",
                );
            })
            .await;
            assert_eq!(
                current_executor_scope(),
                Some(ExecutorScope::Request),
                "the outer scope must be restored",
            );
        })
        .await;
    }

    #[test]
    fn executor_clone_preserves_the_pool_variant() {
        let p = pool();
        let cloned = p.clone();
        assert!(matches!(p, Executor::Pool(_)));
        assert!(matches!(cloned, Executor::Pool(_)));
    }

    #[tokio::test]
    async fn is_mock_connection_forwards_to_inner_on_pool() {
        let executor = pool();
        assert!(!executor.is_mock_connection());
    }

    #[tokio::test]
    async fn no_ambient_state_outside_any_scope_remains_observable() {
        assert!(current_executor().is_none());
        assert!(current_executor_scope().is_none());
    }
}

#[cfg(test)]
mod ambient_tests {
    use std::any::Any;
    use std::sync::Arc;

    use nest_rs_database::{Executor as DynExecutor, with_request_executor};

    struct ForeignHandle;

    impl DynExecutor for ForeignHandle {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[tokio::test]
    async fn a_foreign_ambient_executor_is_reported_rather_than_read_as_absent() {
        let logs = nest_rs_testing::LogCapture::install();
        with_request_executor(Arc::new(ForeignHandle) as Arc<dyn DynExecutor>, async {
            assert!(
                super::current_executor().is_none(),
                "a handle this crate did not install is not a SeaORM executor",
            );
        })
        .await;

        let event = logs.expect_one(crate::TARGET, "ambient executor is not a SeaORM Executor");
        assert_eq!(event.level, "error");
        assert_eq!(
            event.field("reason").as_deref(),
            Some("executor_downcast_miss"),
        );
    }
}

#[cfg(test)]
mod after_commit_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use nest_rs_authz::{AbilityBuilder, current_ability, with_ability};
    use nest_rs_testing::LogCapture;
    use sea_orm::DatabaseConnection;

    use super::*;

    fn boundary() -> Arc<LazyTransaction> {
        Arc::new(LazyTransaction::new(DatabaseConnection::default(), "test"))
    }

    fn flag() -> (Arc<AtomicBool>, impl Future<Output = ()> + Send + 'static) {
        let flag = Arc::new(AtomicBool::new(false));
        let set = flag.clone();
        (flag, async move { set.store(true, Ordering::SeqCst) })
    }

    #[derive(Default)]
    struct Seen {
        on_pool: bool,
        scope: Option<ExecutorScope>,
        ability: Option<Arc<nest_rs_authz::Ability>>,
    }

    #[tokio::test]
    async fn held_work_runs_once_the_boundary_succeeds_with_the_emitters_scope_and_ability() {
        let lazy = boundary();
        let ability = Arc::new(AbilityBuilder::new().build().expect("an empty ability"));
        let seen = Arc::new(Mutex::new(None::<Seen>));
        let record = seen.clone();
        let work = async move {
            *record.lock().unwrap_or_else(PoisonError::into_inner) = Some(Seen {
                on_pool: matches!(current_executor(), Some(Executor::Pool(_))),
                scope: current_executor_scope(),
                ability: current_ability(),
            });
        };

        with_request_executor(
            Executor::Lazy(lazy.clone()),
            with_ability(ability.clone(), nest_rs_database::after_commit(work)),
        )
        .await;
        assert!(
            seen.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_none(),
            "nothing runs while the boundary is open",
        );

        assert!(matches!(
            lazy.finalize(true).await,
            FinalizeOutcome::NoTransaction
        ));
        let seen = seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .expect("the held work ran once the boundary settled");
        assert!(
            seen.on_pool,
            "it runs outside the transaction it waited for"
        );
        assert_eq!(seen.scope, Some(ExecutorScope::Request));
        assert!(
            seen.ability
                .is_some_and(|seen| Arc::ptr_eq(&seen, &ability)),
            "it runs with the ability it was registered under",
        );
    }

    #[tokio::test]
    async fn held_work_is_dropped_unrun_when_the_boundary_fails() {
        let logs = LogCapture::install();
        let lazy = boundary();
        let (ran, work) = flag();

        with_job_executor(
            Executor::Lazy(lazy.clone()),
            nest_rs_database::after_commit(work),
        )
        .await;
        assert!(matches!(
            lazy.finalize(false).await,
            FinalizeOutcome::NoTransaction
        ));

        assert!(!ran.load(Ordering::SeqCst));
        let line = logs.expect_one(
            crate::TARGET,
            "after-commit work discarded: the boundary committed nothing",
        );
        assert_eq!(line.level, "debug");
        assert_eq!(line.field("discarded").as_deref(), Some("1"));
        assert_eq!(line.field("transport").as_deref(), Some("test"));
    }

    #[tokio::test]
    async fn work_reaching_a_boundary_that_already_settled_is_refused_and_said() {
        let logs = LogCapture::install();
        let lazy = boundary();
        let escaped = Executor::Lazy(lazy.clone());
        let (before, held) = flag();
        with_request_executor(escaped.clone(), nest_rs_database::after_commit(held)).await;

        assert!(matches!(
            lazy.finalize(true).await,
            FinalizeOutcome::Escaped
        ));
        let (after, late) = flag();
        assert!(
            nest_rs_database::Executor::after_commit(&escaped, Box::pin(late)).is_none(),
            "a refused piece of work is not handed back to run at once either",
        );

        assert!(!before.load(Ordering::SeqCst));
        assert!(!after.load(Ordering::SeqCst));
        let refused = logs.expect_one(
            crate::TARGET,
            "after-commit work registered on a boundary that has already settled; it will not run",
        );
        assert_eq!(refused.level, "warn");
        assert_eq!(refused.field("outcome").as_deref(), Some("discarded"));
    }

    #[tokio::test]
    async fn a_panicking_piece_of_held_work_is_contained_and_the_rest_run() {
        let logs = LogCapture::install();
        let lazy = boundary();
        let (ran, after) = flag();
        with_request_executor(Executor::Lazy(lazy.clone()), async {
            nest_rs_database::after_commit(async { panic!("after-commit boom") }).await;
            nest_rs_database::after_commit(after).await;
        })
        .await;

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = lazy.finalize(true).await;
        std::panic::set_hook(previous);

        assert!(matches!(outcome, FinalizeOutcome::NoTransaction));
        assert!(ran.load(Ordering::SeqCst), "the work after the panic ran");
        let line = logs.expect_one(
            crate::TARGET,
            "after-commit work panicked; the transaction had already committed",
        );
        assert_eq!(line.level, "error");
        assert_eq!(
            line.field(nest_rs_core::panic::FIELD).as_deref(),
            Some("after-commit boom")
        );
    }

    #[tokio::test]
    async fn held_work_is_dropped_with_an_abandoned_boundary() {
        let logs = LogCapture::install();
        let lazy = boundary();
        let (ran, work) = flag();
        with_request_executor(
            Executor::Lazy(lazy.clone()),
            nest_rs_database::after_commit(work),
        )
        .await;
        drop(lazy);

        assert!(!ran.load(Ordering::SeqCst));
        let line = logs.expect_one(
            crate::TARGET,
            "after-commit work discarded: the boundary committed nothing",
        );
        assert_eq!(line.field("discarded").as_deref(), Some("1"));
    }

    #[test]
    fn a_pool_hands_the_work_back_to_run_at_once() {
        let pool = Executor::Pool(DatabaseConnection::default());
        assert!(nest_rs_database::Executor::after_commit(&pool, Box::pin(async {})).is_some());
    }
}
