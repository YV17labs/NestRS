//! An enum rather than a trait object: `ConnectionTrait` has generic methods,
//! so it is not object-safe.

use std::any::Any;
use std::future::Future;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use nest_rs_core::{Continuation, RequestContinuation};
use nest_rs_database::{AtomicBlock, Deferred};

use crate::atomic::AtomicExecutor;
use crate::error::CommitError;
use crate::savepoint::Savepoint;
use crate::turn::Turn;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, ExecResult,
    QueryResult, Statement, TransactionTrait,
};

pub use nest_rs_database::ExecutorScope;
pub use nest_rs_database::current_executor_scope;

/// The label an atomic block's own transaction logs under: it serves no edge.
const ATOMIC: &str = "atomic";

/// The connection a request's queries run against: the shared pool, the
/// per-request [`DatabaseTransaction`], a transaction opened lazily on first
/// use, or the savepoint an atomic block opened inside one. Cheap to clone.
///
/// It can grow within 7.x, so a `match` outside this crate keeps a `_` arm.
#[derive(Clone)]
#[non_exhaustive]
pub enum Executor {
    /// The shared connection pool — safe (read) methods and system/job work run
    /// here, outside any transaction.
    Pool(DatabaseConnection),
    /// A transaction the **caller** opened and installed itself, through
    /// `with_executor`; the framework never builds one.
    ///
    /// Its commit is the caller's, so work handed to
    /// [`after_commit`](nest_rs_database::after_commit) inside it runs at once:
    /// the caller emits after its own `commit`. Inside a unit of work, a
    /// listener reached before it still runs as a unit of its own, never in
    /// this transaction; outside every unit, it runs bare in this one.
    Txn(Arc<DatabaseTransaction>),
    /// A transaction opened on **first data-layer touch**: what
    /// [`SeaOrmUnitOfWork`](crate::SeaOrmUnitOfWork) installs for a unit of
    /// work of its own, so a unit a guard denies (or one that never queries)
    /// costs no `BEGIN`/`ROLLBACK` round-trip at all.
    /// An [`atomic`](fn@nest_rs_database::atomic) block on the pool, or inside a
    /// caller's [`Txn`](Executor::Txn), runs on one of its own.
    Lazy(Arc<LazyTransaction>),
    /// The savepoint an [`atomic`](fn@nest_rs_database::atomic) block opened
    /// inside a [`Lazy`](Executor::Lazy) transaction or another block's
    /// savepoint, sent with the block's first statement.
    Savepoint(Arc<Savepoint>),
}

/// The deferred-`BEGIN` state behind [`Executor::Lazy`]: what to open on, and
/// the transaction once something touched the data layer. Concurrent first
/// touches race through a [`tokio::sync::OnceCell`], so exactly one
/// transaction opens per request.
pub struct LazyTransaction {
    opener: Opener,
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
    /// landed. An atomic block's statements poison the block, never this
    /// boundary; its `SAVEPOINT`, `RELEASE` and `ROLLBACK TO` poison it.
    poisoned: AtomicU8,
    /// The work [`after_commit`](nest_rs_database::after_commit) handed this
    /// boundary: run by [`finalize`](Self::finalize) once the transaction has
    /// committed, dropped unrun when it has not. `None` once settling begins, so
    /// an escaped handle's late registration is refused.
    after_commit: Mutex<Option<Vec<Deferred>>>,
    /// Held by an open atomic block from its `SAVEPOINT` until it settles.
    turn: Turn,
    /// The outbox a push inside this transaction writes its jobs into, when
    /// the binding that opened it keeps one.
    #[cfg(feature = "queue")]
    outbox: Option<Arc<crate::queue::SeaOrmOutbox>>,
}

/// What a [`LazyTransaction`] sends its `BEGIN` on.
enum Opener {
    /// A transaction of its own on the pool.
    Pool(DatabaseConnection),
    /// A savepoint nested in a transaction the caller opened, through sea-orm's
    /// own nesting: an atomic block inside an [`Executor::Txn`].
    Caller(Arc<DatabaseTransaction>),
}

/// No statement on this transaction has failed.
pub(crate) const POISON_CLEAN: u8 = 0;
/// One failed on something a re-run would hit again — a constraint violation, a
/// type error, a missing relation.
pub(crate) const POISON_DETERMINISTIC: u8 = 1;
/// One failed on something a fresh attempt could clear
/// ([`is_transient_failure`]) — a serialization conflict, a deadlock, or the
/// connection going away before anything could be committed.
///
/// [`is_transient_failure`]: crate::retry::is_transient_failure
pub(crate) const POISON_TRANSIENT: u8 = 2;

impl LazyTransaction {
    /// A lazy transaction over `pool` — nothing is opened yet. `transport` names
    /// the edge for every event this boundary emits.
    pub fn new(pool: DatabaseConnection, transport: &'static str) -> Self {
        Self::opening(Opener::Pool(pool), transport)
    }

    /// An atomic block inside a caller's own transaction: its `BEGIN` is a
    /// `SAVEPOINT` there, its commit a `RELEASE`.
    pub(crate) fn nested_in(caller: Arc<DatabaseTransaction>, transport: &'static str) -> Self {
        Self::opening(Opener::Caller(caller), transport)
    }

    fn opening(opener: Opener, transport: &'static str) -> Self {
        Self {
            opener,
            transport,
            cell: tokio::sync::OnceCell::new(),
            settled: std::sync::atomic::AtomicBool::new(false),
            poisoned: AtomicU8::new(POISON_CLEAN),
            after_commit: Mutex::new(Some(Vec::new())),
            turn: Turn::new(),
            #[cfg(feature = "queue")]
            outbox: None,
        }
    }

    /// The same boundary, writing a push's jobs into `outbox` once a relay
    /// armed it.
    #[cfg(feature = "queue")]
    pub(crate) fn keeping(mut self, outbox: Option<Arc<crate::queue::SeaOrmOutbox>>) -> Self {
        self.outbox = outbox;
        self
    }

    /// The outbox a push inside this boundary writes into: kept by the binding
    /// and armed by a relay.
    #[cfg(feature = "queue")]
    pub(crate) fn outbox(&self) -> Option<&Arc<crate::queue::SeaOrmOutbox>> {
        self.outbox.as_ref().filter(|outbox| outbox.is_armed())
    }

    /// The pool this boundary draws from; a caller's transaction names none.
    pub(crate) fn pool(&self) -> Option<&DatabaseConnection> {
        match &self.opener {
            Opener::Pool(pool) => Some(pool),
            Opener::Caller(_) => None,
        }
    }

    /// The pool handle whatever leaves this boundary runs on — its
    /// after-commit work, a step out of its transaction — keeping the
    /// binding's outbox for a block of its own and a cancel; `None` for a
    /// caller's transaction.
    pub(crate) fn pool_handle(&self) -> Option<Arc<dyn nest_rs_database::Executor>> {
        let pool = self.pool()?.clone();
        #[cfg(feature = "queue")]
        if let Some(outbox) = &self.outbox {
            return Some(Arc::new(crate::queue::KeepingPool::new(
                pool,
                Arc::clone(outbox),
            )));
        }
        Some(Arc::new(Executor::Pool(pool)))
    }

    pub(crate) fn backend(&self) -> DbBackend {
        match &self.opener {
            Opener::Pool(pool) => pool.get_database_backend(),
            Opener::Caller(caller) => caller.get_database_backend(),
        }
    }

    fn support_returning(&self) -> bool {
        match &self.opener {
            Opener::Pool(pool) => pool.support_returning(),
            Opener::Caller(caller) => caller.support_returning(),
        }
    }

    fn is_mock_connection(&self) -> bool {
        match &self.opener {
            Opener::Pool(pool) => pool.is_mock_connection(),
            Opener::Caller(caller) => caller.is_mock_connection(),
        }
    }

    pub(crate) fn transport(&self) -> &'static str {
        self.transport
    }

    pub(crate) fn turn(&self) -> &Turn {
        &self.turn
    }

    /// Keep `work` for after the commit, to run under what the caller carries
    /// into a [`Commit`](Continuation::Commit): the **pool** under the
    /// boundary's scope, the caller's ability, every value that crosses.
    ///
    /// The capture holds the pool, never this `LazyTransaction`: a held `Arc`
    /// to the boundary would make every boundary that held work read as
    /// escaped.
    fn hold(&self, work: Deferred) {
        self.adopt([held_for_commit(work, self.pool_handle())]);
    }

    /// Keep work already captured for its commit — an atomic block's, released
    /// into this boundary.
    pub(crate) fn adopt(&self, work: impl IntoIterator<Item = Deferred>) {
        adopt_into(&self.after_commit, self.transport, work);
    }

    /// The held work, taken for settling — after which nothing more is held.
    fn take_after_commit(&self) -> Vec<Deferred> {
        take_held(&self.after_commit)
    }

    /// Run one statement on the request's transaction, in its turn.
    async fn run<'a, T, F, Fut>(&'a self, statement: F) -> Result<T, DbErr>
    where
        F: FnOnce(&'a DatabaseTransaction) -> Fut,
        Fut: Future<Output = Result<T, DbErr>> + 'a,
    {
        let _turn = self.turn.for_statement().await?;
        self.run_unturned(statement).await
    }

    /// Run one statement on the request's transaction, poisoning the boundary
    /// on **either** failure: a `?` on `txn_ref` would let a failed `BEGIN`
    /// escape the flag. The turn is the caller's: an atomic block sends its
    /// `SAVEPOINT` here while it holds it.
    pub(crate) async fn run_unturned<'a, T, F, Fut>(&'a self, statement: F) -> Result<T, DbErr>
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
            .get_or_try_init(|| async {
                match &self.opener {
                    Opener::Pool(pool) => pool.begin().await,
                    Opener::Caller(caller) => caller.begin().await,
                }
                .map(Arc::new)
            })
            .await?
            .as_ref())
    }

    /// The transaction a data-layer touch opened, if any.
    pub(crate) fn opened(&self) -> Option<&DatabaseTransaction> {
        self.cell.get().map(AsRef::as_ref)
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
        record_failure(&self.poisoned, result)
    }

    /// Leave the boundary unable to commit, whatever a statement says next: an
    /// atomic block left its savepoint in a state nothing can vouch for.
    pub(crate) fn poison_deterministic(&self) {
        poison_once(&self.poisoned, POISON_DETERMINISTIC);
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
        let (outcome, held) = self.answer(success).await;
        held.run().await;
        outcome
    }

    /// [`finalize`](Self::finalize)'s first half: the transaction's answer, and
    /// the work held for the commit, to run or drop once that answer is told.
    pub(crate) async fn answer(self: Arc<Self>, success: bool) -> (FinalizeOutcome, Held) {
        let transport = self.transport;
        // Settling consumes `self` before `COMMIT` is awaited, so its `Drop`
        // cannot report a future dropped mid-`COMMIT`: this guard does.
        let mut abandoned = AbandonedDuringSettle {
            transport,
            armed: self.is_opened(),
        };
        self.settled.store(true, Ordering::Relaxed);
        let work = self.take_after_commit();
        let outcome = Self::settle(self, success).await;
        abandoned.armed = false;
        let committed = match outcome {
            FinalizeOutcome::Committed => true,
            FinalizeOutcome::NoTransaction => success,
            _ => false,
        };
        (
            outcome,
            Held {
                transport,
                work,
                committed,
            },
        )
    }

    /// Whether this boundary holds work for its commit.
    pub(crate) fn holds_work(&self) -> bool {
        self.after_commit
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|held| !held.is_empty())
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
                    target: crate::target::ORM,
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
                    target: crate::target::ORM,
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
                    target: crate::target::ORM,
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
                    target: crate::target::ORM,
                    transport,
                    error = %nest_rs_core::error_message(&err),
                    "poisoned transaction rollback failed"
                );
            }
            tracing::error!(
                target: crate::target::ORM,
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
                    target: crate::target::ORM,
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
        report_discarded(self.transport, still_held(&mut self.after_commit));
        if self.settled.load(Ordering::Relaxed) || self.cell.get().is_none() {
            return;
        }
        report_abandoned(self.transport);
    }
}

/// `work`, wrapped to run under what its unit carries into a
/// [`Commit`](Continuation::Commit), captured here where it is registered — the
/// scope, the unit's binding, the ability, every value that crosses — with the
/// boundary's own `pool` handle installed inside, outside the transaction it
/// waited for; bare without one.
pub(crate) fn held_for_commit(
    work: Deferred,
    pool: Option<Arc<dyn nest_rs_database::Executor>>,
) -> Deferred {
    let carried = RequestContinuation::capture(Continuation::Commit);
    match (carried, pool) {
        (Some(carried), Some(pool)) => {
            Box::pin(carried.scope(nest_rs_database::with_executor(pool, work)))
        }
        (Some(carried), None) => Box::pin(carried.scope(work)),
        (None, Some(pool)) => Box::pin(nest_rs_database::with_executor(pool, work)),
        (None, None) => work,
    }
}

/// Keep captured `work` in `held`, or refuse it once settling took the list.
pub(crate) fn adopt_into(
    held: &Mutex<Option<Vec<Deferred>>>,
    transport: &'static str,
    work: impl IntoIterator<Item = Deferred>,
) {
    let refused = {
        let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
        match held.as_mut() {
            Some(held) => {
                held.extend(work);
                None
            }
            None => Some(work),
        }
    };
    if refused.map_or(0, |work| work.into_iter().count()) > 0 {
        tracing::warn!(
            target: crate::target::ORM,
            transport,
            outcome = "discarded",
            "after-commit work registered on a boundary that has already settled; \
             it will not run",
        );
    }
}

/// The held work, taken for settling — after which nothing more is held.
pub(crate) fn take_held(held: &Mutex<Option<Vec<Deferred>>>) -> Vec<Deferred> {
    held.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .unwrap_or_default()
}

/// How much work `held` still keeps, read as its holder is dropped.
pub(crate) fn still_held(held: &mut Mutex<Option<Vec<Deferred>>>) -> usize {
    held.get_mut()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map_or(0, Vec::len)
}

/// Record in `flag` that a statement failed, and what the database said about
/// it: the first failure wins, since everything after it fails with `25P02`.
pub(crate) fn record_failure<T>(flag: &AtomicU8, result: Result<T, DbErr>) -> Result<T, DbErr> {
    if let Err(err) = &result {
        // A statement, not a `COMMIT`: nothing is durable yet, so the broad
        // predicate is safe here.
        let verdict = if crate::retry::is_transient_failure(err) {
            POISON_TRANSIENT
        } else {
            POISON_DETERMINISTIC
        };
        poison_once(flag, verdict);
    }
    result
}

/// Set `flag` to `verdict` unless an earlier failure already set it.
pub(crate) fn poison_once(flag: &AtomicU8, verdict: u8) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the first failure's verdict stands; a later one finds the slot taken"
    )]
    let _ = flag.compare_exchange(POISON_CLEAN, verdict, Ordering::Relaxed, Ordering::Relaxed);
}

/// The work a settled boundary held for its commit, and whether it committed.
pub(crate) struct Held {
    transport: &'static str,
    work: Vec<Deferred>,
    committed: bool,
}

impl Held {
    /// Run the work when the boundary committed, or drop it.
    pub(crate) async fn run(self) {
        settle_after_commit(self.transport, self.work, self.committed).await;
    }
}

/// Run the boundary's after-commit work when it `committed`, or drop it. A
/// panic is contained: unwinding would turn a committed attempt into a failed
/// one a queue replays.
pub(crate) async fn settle_after_commit(
    transport: &'static str,
    held: Vec<Deferred>,
    committed: bool,
) {
    if !committed {
        report_discarded(transport, held.len());
        return;
    }
    for work in held {
        if let Err(payload) = nest_rs_core::panic::contain(work).await {
            nest_rs_core::contained_panic!(
                target: crate::target::ORM,
                payload.as_ref(),
                "after-commit work panicked; the transaction had already committed",
                transport,
            );
        }
    }
}

/// `debug`: the expected consequence of a rollback the edge already reports.
pub(crate) fn report_discarded(transport: &'static str, discarded: usize) {
    if discarded > 0 {
        tracing::debug!(
            target: crate::target::ORM,
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
        target: crate::target::ORM,
        transport,
        outcome = "abandoned",
        "transaction abandoned without settling; its locks are held until the \
         abandoned statement drains",
    );
}

/// How [`LazyTransaction::finalize`] settled the boundary's transaction.
///
/// It can grow within 7.x, so a `match` outside this crate keeps a `_` arm —
/// one that fails the boundary, since only the arms it names are known to have
/// kept what the boundary wrote:
///
/// ```compile_fail,E0004
/// use nest_rs_seaorm::FinalizeOutcome;
///
/// fn kept(outcome: FinalizeOutcome) -> bool {
///     match outcome {
///         FinalizeOutcome::NoTransaction | FinalizeOutcome::Committed => true,
///         FinalizeOutcome::RolledBack
///         | FinalizeOutcome::Escaped
///         | FinalizeOutcome::CommitFailed(_)
///         | FinalizeOutcome::Poisoned { .. } => false,
///     }
/// }
/// ```
#[derive(Debug)]
#[non_exhaustive]
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
            Executor::Lazy(l) => l.backend(),
            Executor::Savepoint(s) => s.root().backend(),
        }
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        match self {
            Executor::Pool(c) => c.execute_raw(stmt).await,
            Executor::Txn(t) => t.execute_raw(stmt).await,
            Executor::Lazy(l) => l.run(|txn| txn.execute_raw(stmt)).await,
            Executor::Savepoint(s) => s.run(|txn| txn.execute_raw(stmt)).await,
        }
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        match self {
            Executor::Pool(c) => c.execute_unprepared(sql).await,
            Executor::Txn(t) => t.execute_unprepared(sql).await,
            Executor::Lazy(l) => l.run(|txn| txn.execute_unprepared(sql)).await,
            Executor::Savepoint(s) => s.run(|txn| txn.execute_unprepared(sql)).await,
        }
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        match self {
            Executor::Pool(c) => c.query_one_raw(stmt).await,
            Executor::Txn(t) => t.query_one_raw(stmt).await,
            Executor::Lazy(l) => l.run(|txn| txn.query_one_raw(stmt)).await,
            Executor::Savepoint(s) => s.run(|txn| txn.query_one_raw(stmt)).await,
        }
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        match self {
            Executor::Pool(c) => c.query_all_raw(stmt).await,
            Executor::Txn(t) => t.query_all_raw(stmt).await,
            Executor::Lazy(l) => l.run(|txn| txn.query_all_raw(stmt)).await,
            Executor::Savepoint(s) => s.run(|txn| txn.query_all_raw(stmt)).await,
        }
    }

    fn support_returning(&self) -> bool {
        match self {
            Executor::Pool(c) => c.support_returning(),
            Executor::Txn(t) => t.support_returning(),
            Executor::Lazy(l) => l.support_returning(),
            Executor::Savepoint(s) => s.root().support_returning(),
        }
    }

    fn is_mock_connection(&self) -> bool {
        match self {
            Executor::Pool(c) => c.is_mock_connection(),
            Executor::Txn(t) => t.is_mock_connection(),
            Executor::Lazy(l) => l.is_mock_connection(),
            Executor::Savepoint(s) => s.root().is_mock_connection(),
        }
    }
}

impl Executor {
    /// The pool this handle's statements run on, a block's or a lazy
    /// transaction's included; `None` inside a caller's own transaction.
    pub(crate) fn pool(&self) -> Option<&DatabaseConnection> {
        match self {
            Executor::Pool(pool) => Some(pool),
            Executor::Lazy(lazy) => lazy.pool(),
            Executor::Savepoint(block) => block.root().pool(),
            Executor::Txn(_) => None,
        }
    }

    /// The transaction of its own an atomic block on this pool runs on; `None`
    /// on a handle that is no pool.
    pub(crate) fn own_block(&self) -> Option<LazyTransaction> {
        match self {
            Executor::Pool(pool) => Some(LazyTransaction::new(pool.clone(), ATOMIC)),
            _ => None,
        }
    }

    /// The armed outbox a push on this handle writes into: its unit's, through
    /// every block nested in it.
    #[cfg(feature = "queue")]
    fn outbox(&self) -> Option<&Arc<crate::queue::SeaOrmOutbox>> {
        match self {
            Executor::Lazy(lazy) => lazy.outbox(),
            Executor::Savepoint(block) => block.root().outbox(),
            Executor::Pool(_) | Executor::Txn(_) => None,
        }
    }

    /// Run a removal of a kept record on this handle's transaction, in its
    /// turn, answering whether it deleted one.
    #[cfg(feature = "queue")]
    async fn remove_kept(&self, removal: Statement) -> Result<bool, nest_rs_database::OutboxError> {
        crate::queue::removed(self.execute_raw(removal).await)
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
            Executor::Lazy(lazy) if !lazy.is_opened() => lazy.pool_handle(),
            _ => None,
        }
    }

    /// A lazy transaction or a block's savepoint holds the work for its
    /// settling; a pool and a caller's [`Txn`](Executor::Txn) hand it back to
    /// run at once.
    fn after_commit(&self, work: Deferred) -> Option<Deferred> {
        match self {
            Executor::Lazy(lazy) => {
                lazy.hold(work);
                None
            }
            Executor::Savepoint(block) => {
                block.hold(work);
                None
            }
            Executor::Pool(_) | Executor::Txn(_) => Some(work),
        }
    }

    /// A lazy transaction or a block's savepoint writes the records on its
    /// transaction, in its turn, and wakes the relay once it committed; a pool
    /// and a caller's [`Txn`](Executor::Txn) keep none, so the jobs are filed at
    /// once.
    #[cfg(feature = "queue")]
    fn write_outbox(
        &self,
        records: &mut dyn Iterator<Item = nest_rs_database::OutboxRecord>,
    ) -> Option<futures_util::future::BoxFuture<'_, Result<(), nest_rs_database::OutboxError>>>
    {
        let outbox = Arc::clone(self.outbox()?);
        let records: Vec<_> = records.collect();
        Some(Box::pin(async move {
            crate::queue::insert(records, |statement| self.execute_raw(statement)).await?;
            let wake: Deferred = Box::pin(async move { outbox.wake() });
            match self {
                Executor::Lazy(lazy) => lazy.hold(wake),
                Executor::Savepoint(block) => block.hold(wake),
                Executor::Pool(_) | Executor::Txn(_) => {}
            }
            Ok(())
        }))
    }

    #[cfg(feature = "queue")]
    fn remove_outbox<'a>(
        &'a self,
        queue: &'a str,
        id: uuid::Uuid,
    ) -> Option<futures_util::future::BoxFuture<'a, Result<bool, nest_rs_database::OutboxError>>>
    {
        self.outbox()?;
        Some(Box::pin(
            self.remove_kept(crate::queue::removal_of_job(queue, id)),
        ))
    }

    #[cfg(feature = "queue")]
    fn remove_outbox_unique<'a>(
        &'a self,
        queue: &'a str,
        key: &'a str,
    ) -> Option<futures_util::future::BoxFuture<'a, Result<bool, nest_rs_database::OutboxError>>>
    {
        self.outbox()?;
        Some(Box::pin(
            self.remove_kept(crate::queue::removal_of_key(queue, key)),
        ))
    }

    /// A block on the pool is a transaction of its own; inside a caller's
    /// [`Txn`](Executor::Txn), a savepoint sea-orm nests there; inside a lazy
    /// transaction or another block, a savepoint of its own sent on the same
    /// connection.
    fn atomic(&self) -> Option<Arc<dyn AtomicBlock>> {
        let block = match self {
            Executor::Pool(pool) => {
                AtomicExecutor::own(Arc::new(LazyTransaction::new(pool.clone(), ATOMIC)))
            }
            Executor::Txn(caller) => AtomicExecutor::own(Arc::new(LazyTransaction::nested_in(
                Arc::clone(caller),
                ATOMIC,
            ))),
            Executor::Lazy(lazy) => {
                AtomicExecutor::savepoint(Arc::new(Savepoint::in_boundary(Arc::clone(lazy))))
            }
            Executor::Savepoint(block) => {
                AtomicExecutor::savepoint(Arc::new(Savepoint::in_block(Arc::clone(block))))
            }
        };
        Some(Arc::new(block))
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
                target: crate::target::ORM,
                reason = "executor_downcast_miss",
                "ambient executor is not a SeaORM Executor"
            );
            None
        }
    }
}

/// Install `executor` without tagging a scope: with no ambient ability `Repo`
/// denies every row, as on a request; only [`with_job_executor`] is unscoped.
pub fn with_executor<F: Future>(executor: Executor, fut: F) -> impl Future<Output = F::Output> {
    nest_rs_database::with_executor(Arc::new(executor), fut)
}

/// Install `executor` tagged as a **request** scope, so `Repo` applies the
/// ambient ability's `WHERE` (fail-closed with no ability, like any request).
pub fn with_request_executor<F: Future>(
    executor: Executor,
    fut: F,
) -> impl Future<Output = F::Output> {
    nest_rs_database::with_request_executor(Arc::new(executor), fut)
}

/// Install `executor` tagged as a **job** scope — the one unscoped path, for
/// system work (cron, queue) that has no principal to scope to.
pub fn with_job_executor<F: Future>(executor: Executor, fut: F) -> impl Future<Output = F::Output> {
    nest_rs_database::with_job_executor(Arc::new(executor), fut)
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

    #[test]
    fn an_install_holds_the_unit_it_runs_once() {
        let heavy = || async {
            let held = [0u8; 32 * 1024];
            tokio::task::yield_now().await;
            std::hint::black_box(&held);
        };
        let alone = std::mem::size_of_val(&heavy());
        let installed = [
            (
                "with_executor",
                std::mem::size_of_val(&with_executor(pool(), heavy())),
            ),
            (
                "with_request_executor",
                std::mem::size_of_val(&with_request_executor(pool(), heavy())),
            ),
            (
                "with_job_executor",
                std::mem::size_of_val(&with_job_executor(pool(), heavy())),
            ),
        ];
        for (installer, size) in installed {
            assert!(
                size < alone + 1024,
                "{installer} weighs {size} bytes around a {alone}-byte unit",
            );
        }
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

        let event = logs.expect_one(
            crate::target::ORM,
            "ambient executor is not a SeaORM Executor",
        );
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

    /// What the pool a boundary keeping an outbox hands on owes: `Repo` reads
    /// the pool, a push there is filed at once, while a block of its own keeps
    /// its push and a cancel reaches the outbox.
    #[cfg(feature = "queue")]
    fn keeps_the_outbox_for_a_block_and_a_cancel(handle: &dyn nest_rs_database::Executor) {
        let unread = || {
            std::iter::from_fn(|| -> Option<nest_rs_database::OutboxRecord> {
                panic!("a push on the pool builds no record")
            })
        };
        assert!(matches!(
            handle.as_any().downcast_ref::<Executor>(),
            Some(Executor::Pool(_))
        ));
        assert!(handle.write_outbox(&mut unread()).is_none());
        assert!(handle.remove_outbox("notify", uuid::Uuid::nil()).is_some());
        assert!(handle.remove_outbox_unique("notify", "key").is_some());
        let block = handle.atomic().expect("a block of its own");
        let mut record = std::iter::once(nest_rs_database::OutboxRecord::new(
            "notify",
            uuid::Uuid::nil(),
            "{}".to_owned(),
            None,
            std::time::Duration::ZERO,
        ));
        assert!(
            block.write_outbox(&mut record).is_some(),
            "the block keeps its push until it commits"
        );
    }

    #[cfg(feature = "queue")]
    #[tokio::test]
    async fn the_pool_a_boundary_hands_on_keeps_its_outbox_and_a_pool_the_caller_built_none() {
        let keeping = || {
            Arc::new(
                LazyTransaction::new(DatabaseConnection::default(), "test")
                    .keeping(Some(crate::queue::armed())),
            )
        };
        let stepped = nest_rs_database::Executor::non_transactional(&Executor::Lazy(keeping()))
            .expect("an unopened boundary steps out");
        keeps_the_outbox_for_a_block_and_a_cancel(&*stepped);

        let lazy = keeping();
        let (ran, set) = flag();
        with_job_executor(
            Executor::Lazy(lazy.clone()),
            nest_rs_database::after_commit(async move {
                let handle = nest_rs_database::current_executor().expect("the boundary's pool");
                keeps_the_outbox_for_a_block_and_a_cancel(&*handle);
                set.await;
            }),
        )
        .await;
        assert!(matches!(
            lazy.finalize(true).await,
            FinalizeOutcome::NoTransaction
        ));
        assert!(ran.load(Ordering::SeqCst), "the held work ran its checks");

        let caller = Executor::Pool(DatabaseConnection::default());
        assert!(
            nest_rs_database::Executor::remove_outbox(&caller, "notify", uuid::Uuid::nil())
                .is_none()
        );
        let block = nest_rs_database::Executor::atomic(&caller).expect("a block of its own");
        let mut unread = std::iter::from_fn(|| -> Option<nest_rs_database::OutboxRecord> {
            panic!("a pool the caller built keeps no outbox")
        });
        assert!(block.write_outbox(&mut unread).is_none());
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
            crate::target::ORM,
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
            crate::target::ORM,
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
            crate::target::ORM,
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
            crate::target::ORM,
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
