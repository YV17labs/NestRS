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
use std::sync::atomic::{AtomicU8, Ordering};

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
    /// A transaction the **caller** opened and installed itself.
    ///
    /// Nothing in the framework constructs this: every boundary it owns —
    /// HTTP, WS, MCP, the worker — installs [`Lazy`](Self::Lazy), so the
    /// `BEGIN` waits for a data-layer touch and a denied request costs no
    /// round-trip. What keeps the variant is the opposite direction: an app
    /// that opens its own `DatabaseTransaction` (a programmatic boundary, a
    /// migration step, a test that wants everything rolled back) installs it
    /// through `with_executor` and gets the whole `Repo` surface running inside
    /// it. Being *given* a transaction is also why `create_from_active`
    /// SAVEPOINTs on it rather than opening one.
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
    /// The edge this boundary serves — declared once, at construction, and read
    /// back by [`finalize`](Self::finalize) and by the [`Drop`] warning. It used
    /// to be an argument to `finalize`, which meant the one path that never
    /// reaches `finalize` had no way to name itself.
    transport: &'static str,
    cell: tokio::sync::OnceCell<Arc<DatabaseTransaction>>,
    /// Set the instant [`finalize`](Self::finalize) begins, so [`Drop`] can tell
    /// a settled boundary from an **abandoned** one.
    settled: std::sync::atomic::AtomicBool,
    /// Set the moment a statement on **this** transaction returns an error, to
    /// [`POISON_TRANSIENT`] or [`POISON_DETERMINISTIC`] according to that error.
    ///
    /// Postgres aborts the whole transaction on the first failed statement and
    /// refuses everything after it (`25P02`), and a `COMMIT` on an aborted
    /// transaction *succeeds* while rolling back. So a boundary that swallows a
    /// `DbErr` and reports success would be told the commit worked and write
    /// nothing — the silent-write-loss this flag exists to refuse. Nested
    /// transactions ([`begin_nested`](Self::begin_nested)) run their statements
    /// on their own handle and never reach here, which is why a `SAVEPOINT`ed
    /// insert that fails still leaves the outer transaction committable.
    /// *Opening* one does reach here: a `SAVEPOINT` that cannot be issued means
    /// the outer transaction is already unusable, which is the opposite case.
    ///
    /// An `AtomicU8` rather than a flag beside a stored `DbErr`: the only thing
    /// any settle site asks of that error is whether repeating the attempt could
    /// end differently, and that answer is one bit every query path can record
    /// without allocating or locking.
    poisoned: AtomicU8,
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
        }
    }

    /// Run one statement on the request's transaction, poisoning the boundary
    /// on **either** failure.
    ///
    /// Opening is a failure like any other, and it used to be the one that got
    /// away: `poison(txn_ref().await?.execute(..).await)` applies `?` first, so
    /// a `BEGIN` that could not be issued — a pool acquire timeout, a restarted
    /// database — returned `Err` with the flag still clean. A job or handler
    /// that swallowed it then reported success over a boundary that had opened
    /// nothing, which is the silent write loss the flag exists to refuse, on the
    /// one path the flag never saw.
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
    ///
    /// Takes the cell rather than destructuring `self`: this type carries a
    /// [`Drop`] now, so it cannot be moved out of piecewise.
    pub fn into_opened(mut self) -> Option<Arc<DatabaseTransaction>> {
        self.cell.take()
    }

    /// Whether a transaction has been opened.
    pub fn is_opened(&self) -> bool {
        self.cell.get().is_some()
    }

    /// Record that a statement on this transaction failed, and what the database
    /// said about it. Every `Executor::Lazy` query path funnels its `Err`
    /// through here, so the classification covers the whole data layer rather
    /// than the call sites that remembered.
    ///
    /// **The first failure wins.** It is the one that aborted the transaction;
    /// everything after it fails with `25P02`, which says nothing about why.
    fn poison<T>(&self, result: Result<T, DbErr>) -> Result<T, DbErr> {
        if let Err(err) = &result {
            // `is_transient_failure`, not `is_retryable_conflict`: this records a
            // **statement**, and inside an open transaction nothing is durable
            // until `COMMIT` — so a pool acquire timeout or a connection the
            // server closed leaves nothing behind and a replay cannot write
            // twice. The commit site keeps the narrow predicate, because there
            // the same error means the `COMMIT` may have landed. See
            // [`is_transient_failure`](crate::retry::is_transient_failure).
            let verdict = if crate::retry::is_transient_failure(err) {
                POISON_TRANSIENT
            } else {
                POISON_DETERMINISTIC
            };
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
    /// already) and start a **SAVEPOINT** on it. The nested transaction lets an
    /// atomic insert + scope re-check roll back independently of the outer
    /// request transaction, so a handler that swallows the denial cannot leave
    /// an out-of-scope row to be committed with the rest of the request
    /// (DATA-S1).
    ///
    /// Through [`run`](Self::run) like every other statement, and for the same
    /// reason: `txn_ref().await?.begin()` applies `?` first, so neither a
    /// `BEGIN` that could not be issued nor a refused `SAVEPOINT` would poison
    /// the boundary. The second one is the dangerous half — Postgres aborts the
    /// outer transaction, and `COMMIT` on an aborted transaction *succeeds*
    /// while rolling back, so a caller that swallows this `DbErr` would be told
    /// its writes landed.
    pub(crate) async fn begin_nested(&self) -> Result<DatabaseTransaction, DbErr> {
        self.run(|txn| txn.begin()).await
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
    pub async fn finalize(self: Arc<Self>, success: bool) -> FinalizeOutcome {
        let transport = self.transport;
        // `Drop` covers the boundary abandoned *before* settling; this covers the
        // window settling itself opens, and both were needed. `into_opened`
        // takes the cell and drops the `LazyTransaction` before the `COMMIT` is
        // awaited, so from that point the `Drop` guard no longer exists — and a
        // future dropped mid-`COMMIT` is the case where the locks are most
        // certainly still held. Armed only when something was opened: with no
        // transaction there is nothing to hold.
        let mut abandoned = AbandonedDuringSettle {
            transport,
            armed: self.is_opened(),
        };
        // Before anything can return early: `Drop` reports an *abandoned*
        // boundary, and every path from here on is a settled one.
        self.settled.store(true, Ordering::Relaxed);
        let outcome = Self::settle(self, success).await;
        abandoned.armed = false;
        outcome
    }

    /// [`finalize`](Self::finalize)'s body, split out so the guard above wraps
    /// every path through it — including the awaits.
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
        // The one combination that loses writes in silence: a statement failed,
        // and the boundary reported success anyway. Asked once — both sites
        // below settle on it, and `Some(retryable)` is the whole answer either
        // has to carry.
        let flag = lazy.poisoned.load(Ordering::Relaxed);
        let poisoned = (success && flag != POISON_CLEAN).then_some(flag == POISON_TRANSIENT);
        let Some(txn) = lazy.into_opened() else {
            // Nothing opened — but something may still have *failed* to open.
            // Reporting `NoTransaction` for a boundary that swallowed a failed
            // `BEGIN` would say "nothing to settle" about work that was meant to
            // land and did not.
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
        // A statement already failed, so the transaction is aborted and its
        // `COMMIT` would succeed having written nothing. Reporting success here
        // is the one outcome that loses writes in silence, so it is refused
        // before the round-trip rather than discovered by its absence.
        if let Some(retryable) = poisoned {
            if let Err(err) = txn.rollback().await {
                tracing::error!(
                    target: crate::TARGET,
                    transport,
                    error = %err,
                    "poisoned transaction rollback failed"
                );
            }
            tracing::error!(
                target: crate::TARGET,
                transport,
                outcome = "rollback_and_fail",
                // The same bit `CommitFailed` reports: without it a `40001` a
                // handler swallowed is indistinguishable in the logs from a
                // constraint violation it swallowed.
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
                    error = %err,
                    "transaction rollback failed"
                );
            }
            FinalizeOutcome::RolledBack
        }
    }
}

impl Drop for LazyTransaction {
    /// The boundary was **abandoned**: the future holding this executor was
    /// dropped before anything settled it.
    ///
    /// That is not hypothetical and not a bug to fix here — it is the
    /// framework's own shutdown path. A queue worker's `shutdown_timeout`
    /// elapsing makes apalis drop the job future wherever it was, and if that
    /// was mid-statement the transaction stays open until the abandoned
    /// statement drains **server-side**: sea-orm's rollback cannot go out until
    /// the connection is free again, so every row lock the attempt took is held
    /// for the rest of that statement, and the connection stays out of the pool.
    /// The framework cannot cancel a statement — that is not in sea-orm's
    /// contract — so what it owes is the event, at the level an operator tuning
    /// a shutdown timeout will actually see.
    ///
    /// Silent when nothing was opened (there is nothing to hold) and when
    /// `finalize` ran (it has already said what happened, in more detail).
    fn drop(&mut self) {
        if self.settled.load(Ordering::Relaxed) || self.cell.get().is_none() {
            return;
        }
        report_abandoned(self.transport);
    }
}

/// Reports a boundary whose *settling* was abandoned — the future awaiting
/// `COMMIT` or `ROLLBACK` was dropped before it returned.
///
/// The same event `LazyTransaction`'s own `Drop` emits, and it has to be a
/// second guard rather than a longer-lived first one: settling consumes the
/// `LazyTransaction`, so by the time the round-trip is in flight there is no
/// `Drop` left to fire. One sentence for both, so an operator greps once.
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

/// The one wording, for the two guards that can raise it.
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
    /// **It carries no "was anything opened yet" flag, and that is a decision.**
    /// `finalize` computes it and logs it as a field, because an operator
    /// reading the event wants it — but the escaped handle is *still live*, so
    /// "nothing opened at the moment of the check" is not a promise that nothing
    /// will be: a spawned task holding the executor can open a transaction and
    /// write seconds later, and those writes are rolled back when it drops.
    /// Reporting `Settled` on the strength of that flag would turn the one
    /// failure this whole mechanism exists to refuse — a success that lost its
    /// writes — into a `warn` nobody reads. So the escape fails closed either
    /// way, and the flag stays out of the type where a reader would take it for
    /// a guarantee.
    Escaped,
    /// The commit itself failed — the caller classifies and logs it.
    CommitFailed(CommitError),
    /// A statement failed inside the transaction, yet the boundary reported
    /// success. Nothing was committed — Postgres aborts the transaction on the
    /// first failed statement, and its `COMMIT` then succeeds while rolling
    /// back, so believing the boundary would report writes that never landed.
    /// Handled exactly like [`CommitFailed`](Self::CommitFailed): the caller
    /// fails an otherwise-successful outcome loudly. Already logged at `error`.
    Poisoned {
        /// Whether the statement that failed did so on something a fresh attempt
        /// could clear, so a caller with a retry budget can tell "this will fail
        /// identically forever" from "this may well win next time". Read from
        /// the failure the database reported, never guessed.
        ///
        /// **It answers for this boundary's transaction, and nothing else.**
        /// `true` says the attempt's *transaction* wrote nothing, which is what
        /// makes replaying it safe. Work the attempt committed elsewhere — an
        /// HTTP call, an S3 write, or a statement deliberately stepped out of
        /// the transaction through
        /// [`non_transactional`](nest_rs_database::Executor::non_transactional)
        /// — is outside that promise and is replayed with the rest of the body.
        /// The step-out is documented as the read-only escape for exactly this
        /// reason.
        retryable: bool,
    },
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
                target: crate::TARGET,
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

#[cfg(test)]
mod ambient_tests {
    use std::any::Any;
    use std::sync::Arc;

    use nest_rs_database::{Executor as DynExecutor, with_request_executor};

    /// Some *other* ORM's handle installed in the shared task-local — the one
    /// shape `current_executor`'s downcast can miss.
    struct ForeignHandle;

    impl DynExecutor for ForeignHandle {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// A downcast miss answers `None`, which is exactly what "no ambient
    /// executor at all" answers — so every caller takes the same fallback and
    /// the request either opens its own connection or denies, quietly.
    ///
    /// That makes this a framework bug that degrades instead of failing, and
    /// the event is the only thing that distinguishes it from an ordinary
    /// unscoped call. `reason` is what a reader greps for.
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
