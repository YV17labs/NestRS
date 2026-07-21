//! Ambient, request-scoped database executor.
//!
//! [`Executor`] is the request's current SeaORM connection — pool or
//! transaction — implementing both SeaORM's `ConnectionTrait` (so it can
//! drive any query) and [`nest_rs_database::Executor`] (so it lives in the
//! ORM-agnostic task-local the request boundary installs and [`Repo`]
//! reads back). The enum exists because `ConnectionTrait` has generic
//! methods (not object-safe); forwarding via a variant restores a single
//! `&Executor` that drives any SeaORM query.
//!
//! The task-local plumbing itself lives in `nest-rs-database`; this module
//! re-exports it with SeaORM-typed convenience signatures so existing
//! callers see no change.
//!
//! [`Repo`]: crate::Repo

use std::any::Any;
use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, ExecResult,
    QueryResult, Statement, TransactionTrait,
};

pub use nest_rs_database::ExecutorScope;
pub use nest_rs_database::current_executor_scope;

/// The connection a request's queries run against: the shared pool, the
/// per-request [`DatabaseTransaction`], or a transaction opened lazily on
/// first use. Cheap to clone.
///
/// `DatabaseConnection` is internally `Arc`-shaped (a `Clone` handle on the
/// connection manager), so the `Pool` variant holds it directly — wrapping
/// it in an outer `Arc` would carry a redundant refcount on every request
/// (the executor is already re-wrapped as `Arc<dyn nest_rs_database::Executor>`
/// when installed in the task-local). `DatabaseTransaction` is **not**
/// internally `Arc`-shaped, so the `Txn` variant keeps its `Arc`.
#[derive(Clone)]
pub enum Executor {
    /// The shared connection pool — safe (read) methods and system/job work run
    /// here, outside any transaction.
    Pool(DatabaseConnection),
    /// The request's transaction — a mutating HTTP method runs here so the
    /// interceptor can commit on success and roll back on failure.
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
    cell: tokio::sync::OnceCell<Arc<DatabaseTransaction>>,
}

impl LazyTransaction {
    /// A lazy transaction over `pool` — nothing is opened yet.
    pub fn new(pool: DatabaseConnection) -> Self {
        Self {
            pool,
            cell: tokio::sync::OnceCell::new(),
        }
    }

    /// The request's transaction, opening it on the first call. Returns a
    /// borrow so the per-query path costs no `Arc` clone.
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
    pub fn into_opened(self) -> Option<Arc<DatabaseTransaction>> {
        self.cell.into_inner()
    }

    /// Whether a transaction has been opened.
    pub fn is_opened(&self) -> bool {
        self.cell.get().is_some()
    }

    /// Force the request transaction open (if a data-layer touch has not
    /// already) and start a **SAVEPOINT** on it. The nested transaction lets an
    /// atomic insert + scope re-check roll back independently of the outer
    /// request transaction, so a handler that swallows the denial cannot leave
    /// an out-of-scope row to be committed with the rest of the request
    /// (DATA-S1).
    pub(crate) async fn begin_nested(&self) -> Result<DatabaseTransaction, DbErr> {
        self.txn_ref().await?.begin().await
    }

    /// Settle the boundary's lazily opened transaction: commit on `success`,
    /// roll back otherwise, do nothing when no data-layer touch ever opened
    /// one. This is the **single home** of the escape invariant — a lingering
    /// executor clone in a spawned task cannot be committed, so it is logged
    /// at `error` and reported as [`FinalizeOutcome::Escaped`]; the caller
    /// must fail an otherwise-successful response loudly rather than lose the
    /// writes silently. A rollback failure is logged here and reported as
    /// rolled back (the transaction is gone either way); a commit failure is
    /// **not** logged here — it is returned so the transport can classify it
    /// (serialization conflicts vs. generic failure).
    pub async fn finalize(
        self: Arc<Self>,
        success: bool,
        transport: &'static str,
    ) -> FinalizeOutcome {
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
                    target: "nest_rs::orm",
                    transport,
                    opened,
                    outcome = escaped_outcome,
                    "executor escaped into a spawned task"
                );
                return FinalizeOutcome::Escaped { opened };
            }
        };
        let Some(txn) = lazy.into_opened() else {
            return FinalizeOutcome::NoTransaction;
        };
        let txn = match Arc::try_unwrap(txn) {
            Ok(txn) => txn,
            Err(escaped) => {
                drop(escaped);
                tracing::error!(
                    target: "nest_rs::orm",
                    transport,
                    opened = true,
                    outcome = escaped_outcome,
                    "transaction escaped into a spawned task"
                );
                return FinalizeOutcome::Escaped { opened: true };
            }
        };
        if success {
            match txn.commit().await {
                Ok(()) => FinalizeOutcome::Committed,
                Err(err) => FinalizeOutcome::CommitFailed(CommitError(err)),
            }
        } else {
            if let Err(err) = txn.rollback().await {
                tracing::error!(
                    target: "nest_rs::orm",
                    transport,
                    error = %err,
                    "transaction rollback failed"
                );
            }
            FinalizeOutcome::RolledBack
        }
    }
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
    Escaped {
        /// Whether a transaction had been opened when the escape was detected.
        opened: bool,
    },
    /// The commit itself failed — the caller classifies and logs it.
    CommitFailed(CommitError),
}

/// A commit-time database failure. Opaque over the ORM's `DbErr` so a sea-orm
/// version bump is not a semver break through [`FinalizeOutcome`] (B-DATA): the
/// public surface exposes only what a caller needs — a [`Display`] for logging
/// and [`is_retryable_conflict`](CommitError::is_retryable_conflict) for
/// classification — never the wrapped `DbErr` itself.
///
/// [`Display`]: std::fmt::Display
#[derive(Debug)]
pub struct CommitError(DbErr);

impl CommitError {
    /// Whether the commit failed on a retryable serialization/deadlock conflict
    /// (a typed SQLSTATE `40001`/`40P01`/…), matched against the *typed* error
    /// rather than message text. The interceptor tags these for observability;
    /// the replay itself belongs at a programmatic transaction boundary
    /// (`retry::retry_on_conflict`).
    pub fn is_retryable_conflict(&self) -> bool {
        crate::retry::is_retryable_conflict(&self.0)
    }
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for CommitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
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
            Executor::Lazy(l) => l.txn_ref().await?.execute_raw(stmt).await,
        }
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        match self {
            Executor::Pool(c) => c.execute_unprepared(sql).await,
            Executor::Txn(t) => t.execute_unprepared(sql).await,
            Executor::Lazy(l) => l.txn_ref().await?.execute_unprepared(sql).await,
        }
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        match self {
            Executor::Pool(c) => c.query_one_raw(stmt).await,
            Executor::Txn(t) => t.query_one_raw(stmt).await,
            Executor::Lazy(l) => l.txn_ref().await?.query_one_raw(stmt).await,
        }
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        match self {
            Executor::Pool(c) => c.query_all_raw(stmt).await,
            Executor::Txn(t) => t.query_all_raw(stmt).await,
            Executor::Lazy(l) => l.txn_ref().await?.query_all_raw(stmt).await,
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

/// Slots the SeaORM `Executor` into the ORM-agnostic ambient task-local.
/// The downcast back to `Executor` is what [`Repo::conn`](crate::Repo::conn)
/// uses to recover a typed handle for SeaORM queries.
impl nest_rs_database::Executor for Executor {
    fn as_any(&self) -> &dyn Any {
        self
    }

    /// Only a lazy transaction **nothing has opened yet** yields a pool handle:
    /// `Pool` is already outside a transaction, `Txn` is a deliberate
    /// programmatic boundary, and once a `BEGIN` has been issued the request
    /// keeps its transaction (stepping out of live transaction state would buy
    /// nothing and split the request's view of the data).
    fn non_transactional(&self) -> Option<Arc<dyn nest_rs_database::Executor>> {
        match self {
            Executor::Lazy(lazy) if !lazy.is_opened() => {
                Some(Arc::new(Executor::Pool(lazy.pool.clone())))
            }
            _ => None,
        }
    }
}

/// The SeaORM `Executor` installed in the ambient task-local for this
/// request or job, or `None` outside any scope. A downcast miss is a
/// framework bug (some other ORM installed its handle in the same
/// task-local) — it logs an error and surfaces as `None`.
pub fn current_executor() -> Option<Executor> {
    let dynamic = nest_rs_database::current_executor()?;
    match dynamic.as_any().downcast_ref::<Executor>() {
        Some(executor) => Some(executor.clone()),
        None => {
            tracing::error!(
                target: "nest_rs::orm",
                reason = "executor_downcast_miss",
                "ambient executor is not a SeaORM Executor"
            );
            None
        }
    }
}

/// Install `executor` without tagging a scope. Prefer the request/job
/// variants at framework boundaries so authorization can distinguish the
/// two paths. An untagged scope is fail-closed under `Repo` (deny-all with no
/// ambient ability, exactly like a request); only [`with_job_executor`] is
/// unscoped.
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
    #[allow(unused_imports)]
    use super::*;

    fn pool() -> Executor {
        Executor::Pool(DatabaseConnection::default())
    }

    /// Downcast a `dyn nest_rs_database::Executor` back to the SeaORM enum.
    fn as_seaorm(executor: &Arc<dyn nest_rs_database::Executor>) -> &Executor {
        executor
            .as_any()
            .downcast_ref::<Executor>()
            .expect("the handle is a SeaORM Executor")
    }

    #[test]
    fn an_unopened_lazy_transaction_hands_out_its_pool() {
        // DATA-S5: the GraphQL endpoint steps a proven read-only operation out
        // of the transaction the POST boundary installed.
        let lazy = Executor::Lazy(Arc::new(
            LazyTransaction::new(DatabaseConnection::default()),
        ));
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
            // `with_executor` is the unspecified-scope variant — guards that
            // gate on scope must see `None` here, not a stale value.
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

    // `with_executor` (the unspecified-scope variant) does not unwind the
    // scope task-local because it never set it — verifies the two task-locals
    // are genuinely independent, not coupled by a wrapping accident.
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

    // Calling `current_executor` twice inside the same scope returns
    // independently-clonable handles — `Executor` is `Clone`, the task-local
    // hands out a clone each time, so a second access does not "consume" it.
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

    // Nested scopes shadow the outer one — typical when a job runs inside a
    // request (e.g. enqueueing in-process). The outer scope is restored on
    // exit; a bug that drops the inner reset would leak the inner executor.
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

    // `Executor` is `Clone`; the variant must round-trip without changing
    // shape. `DatabaseConnection` is already internally `Arc`-shaped, so the
    // clone is a cheap reference bump, no deep copy. Pinning this here so a
    // refactor to a non-`Clone` field surfaces immediately.
    #[test]
    fn executor_clone_preserves_the_pool_variant() {
        let p = pool();
        let cloned = p.clone();
        assert!(matches!(p, Executor::Pool(_)));
        assert!(matches!(cloned, Executor::Pool(_)));
    }

    // `is_mock_connection` is the one `ConnectionTrait` forwarder safe to
    // call on a disconnected `DatabaseConnection`: without the `mock`
    // feature it falls back to the trait default (`false`), so it neither
    // panics nor reaches a sqlx pool. Exercises the `Pool` arm of the
    // forwarding match without needing a live DB.
    #[tokio::test]
    async fn is_mock_connection_forwards_to_inner_on_pool() {
        let executor = pool();
        // A real Postgres pool would still report `false`; the assertion
        // pins the trait-default behaviour, not the value itself.
        assert!(!executor.is_mock_connection());
    }

    #[tokio::test]
    async fn no_ambient_state_outside_any_scope_remains_observable() {
        // Outside any scope, both task-locals stay `None` — the guard the
        // `repo::scope_for` deny-all branch keys on.
        assert!(current_executor().is_none());
        assert!(current_executor_scope().is_none());
    }
}
