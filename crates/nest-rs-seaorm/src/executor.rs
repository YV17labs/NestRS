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
use crate::savepoint::Savepoint;
use crate::turn::Turn;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, ExecResult,
    QueryResult, Statement, TransactionTrait,
};

/// The label an atomic block's own transaction logs under: it serves no edge.
const ATOMIC: &str = "atomic";

/// The connection a request's queries run against: the shared pool, a
/// transaction opened lazily on first use, or the savepoint an atomic block
/// opened inside one. Cheap to clone.
///
/// Tier 2, for this crate's suites: nothing public installs or hands one out.
#[derive(Clone)]
#[non_exhaustive]
pub enum SeaOrmExecutor {
    /// The shared connection pool — safe (read) methods and system/job work run
    /// here, outside any transaction.
    Pool(DatabaseConnection),
    /// A transaction opened on **first data-layer touch**: what
    /// [`SeaOrmUnitOfWork`](crate::SeaOrmUnitOfWork) installs for a unit of
    /// work of its own, so a unit a guard denies (or one that never queries)
    /// costs no `BEGIN`/`ROLLBACK` round-trip at all.
    /// An [`atomic`](fn@nest_rs_database::atomic) block on the pool runs on one
    /// of its own.
    Lazy(Arc<LazyTransaction>),
    /// The savepoint an [`atomic`](fn@nest_rs_database::atomic) block opened
    /// inside a [`Lazy`](SeaOrmExecutor::Lazy) transaction or another block's
    /// savepoint, sent with the block's first statement.
    Savepoint(Arc<Savepoint>),
}

/// The deferred-`BEGIN` state behind [`SeaOrmExecutor::Lazy`]: the pool to open on,
/// and the transaction once something touched the data layer. Concurrent first
/// touches race through a [`tokio::sync::OnceCell`], so exactly one
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
        Self {
            pool,
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

    /// The pool this boundary draws from.
    pub(crate) fn pool(&self) -> &DatabaseConnection {
        &self.pool
    }

    /// The pool handle whatever leaves this boundary runs on — its
    /// after-commit work, a step out of its transaction — keeping the
    /// binding's outbox for a block of its own and a cancel.
    pub(crate) fn pool_handle(&self) -> Arc<dyn nest_rs_database::Executor> {
        let pool = self.pool.clone();
        #[cfg(feature = "queue")]
        if let Some(outbox) = &self.outbox {
            return Arc::new(crate::queue::KeepingPool::new(pool, Arc::clone(outbox)));
        }
        SeaOrmExecutor::Pool(pool).into()
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
            .get_or_try_init(|| async { self.pool.begin().await.map(Arc::new) })
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
                report_rollback_failed(transport, &err, Settled::Poisoned);
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
                Err(err) => FinalizeOutcome::CommitFailed(err),
            }
        } else {
            if let Err(err) = txn.rollback().await {
                report_rollback_failed(transport, &err, Settled::Failed);
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
/// waited for.
pub(crate) fn held_for_commit(
    work: Deferred,
    pool: Arc<dyn nest_rs_database::Executor>,
) -> Deferred {
    let on_pool = nest_rs_database::__private::with_executor(pool, work);
    match RequestContinuation::capture(Continuation::Commit) {
        Some(carried) => Box::pin(carried.scope(on_pool)),
        None => Box::pin(on_pool),
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

/// Why a boundary rolled its transaction back.
#[derive(Clone, Copy)]
enum Settled {
    /// A statement failed inside it while it reported success.
    Poisoned,
    /// It failed.
    Failed,
}

/// Say a rollback the database did not confirm. A statement Postgres
/// cancelled at its bound (`57014`) answers the rollback of the transaction
/// it ran in — a body cut at its deadline left it running — and Postgres has
/// aborted that transaction: the in-doubt end every edge says at `warn`
/// ([`DbOutcome::Unavailable`](crate::DbOutcome::Unavailable)).
fn report_rollback_failed(transport: &'static str, err: &DbErr, why: Settled) {
    let error = nest_rs_core::error_message(err);
    if crate::DbOutcome::of(err) == crate::DbOutcome::Unavailable {
        tracing::warn!(
            target: crate::target::ORM,
            transport,
            outcome = "rollback",
            error = %error,
            "transaction rolled back after a statement Postgres cancelled; nothing it wrote \
             stands"
        );
        return;
    }
    match why {
        Settled::Poisoned => tracing::error!(
            target: crate::target::ORM,
            transport,
            error = %error,
            "poisoned transaction rollback failed"
        ),
        Settled::Failed => tracing::error!(
            target: crate::target::ORM,
            transport,
            error = %error,
            "transaction rollback failed"
        ),
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
    CommitFailed(DbErr),
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
impl ConnectionTrait for SeaOrmExecutor {
    fn get_database_backend(&self) -> DbBackend {
        self.pool().get_database_backend()
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        match self {
            SeaOrmExecutor::Pool(c) => c.execute_raw(stmt).await,
            SeaOrmExecutor::Lazy(l) => l.run(|txn| txn.execute_raw(stmt)).await,
            SeaOrmExecutor::Savepoint(s) => s.run(|txn| txn.execute_raw(stmt)).await,
        }
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        match self {
            SeaOrmExecutor::Pool(c) => c.execute_unprepared(sql).await,
            SeaOrmExecutor::Lazy(l) => l.run(|txn| txn.execute_unprepared(sql)).await,
            SeaOrmExecutor::Savepoint(s) => s.run(|txn| txn.execute_unprepared(sql)).await,
        }
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        match self {
            SeaOrmExecutor::Pool(c) => c.query_one_raw(stmt).await,
            SeaOrmExecutor::Lazy(l) => l.run(|txn| txn.query_one_raw(stmt)).await,
            SeaOrmExecutor::Savepoint(s) => s.run(|txn| txn.query_one_raw(stmt)).await,
        }
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        match self {
            SeaOrmExecutor::Pool(c) => c.query_all_raw(stmt).await,
            SeaOrmExecutor::Lazy(l) => l.run(|txn| txn.query_all_raw(stmt)).await,
            SeaOrmExecutor::Savepoint(s) => s.run(|txn| txn.query_all_raw(stmt)).await,
        }
    }

    fn support_returning(&self) -> bool {
        self.pool().support_returning()
    }

    fn is_mock_connection(&self) -> bool {
        self.pool().is_mock_connection()
    }
}

impl SeaOrmExecutor {
    /// The pool this handle's statements run on, a block's or a lazy
    /// transaction's included.
    pub(crate) fn pool(&self) -> &DatabaseConnection {
        match self {
            SeaOrmExecutor::Pool(pool) => pool,
            SeaOrmExecutor::Lazy(lazy) => lazy.pool(),
            SeaOrmExecutor::Savepoint(block) => block.root().pool(),
        }
    }

    /// The transaction of its own an atomic block on this pool runs on; `None`
    /// on a handle that is no pool.
    pub(crate) fn own_block(&self) -> Option<LazyTransaction> {
        match self {
            SeaOrmExecutor::Pool(pool) => Some(LazyTransaction::new(pool.clone(), ATOMIC)),
            _ => None,
        }
    }

    /// The armed outbox a push on this handle writes into: its unit's, through
    /// every block nested in it.
    #[cfg(feature = "queue")]
    fn outbox(&self) -> Option<&Arc<crate::queue::SeaOrmOutbox>> {
        match self {
            SeaOrmExecutor::Lazy(lazy) => lazy.outbox(),
            SeaOrmExecutor::Savepoint(block) => block.root().outbox(),
            SeaOrmExecutor::Pool(_) => None,
        }
    }

    /// Run a removal of a kept record on this handle's transaction, in its
    /// turn, answering whether it deleted one.
    #[cfg(feature = "queue")]
    async fn remove_kept(&self, removal: Statement) -> Result<bool, nest_rs_database::OutboxError> {
        crate::queue::removed(self.execute_raw(removal).await)
    }

    /// Only a lazy transaction **nothing has opened yet** yields a pool handle:
    /// once `BEGIN` went out, stepping out would split the request's view of
    /// the data.
    pub(crate) fn non_transactional(&self) -> Option<Arc<dyn nest_rs_database::Executor>> {
        match self {
            SeaOrmExecutor::Lazy(lazy) if !lazy.is_opened() => Some(lazy.pool_handle()),
            _ => None,
        }
    }

    /// A lazy transaction or a block's savepoint holds the work for its
    /// settling; a pool hands it back to run at once.
    pub(crate) fn after_commit(&self, work: Deferred) -> Option<Deferred> {
        match self {
            SeaOrmExecutor::Lazy(lazy) => {
                lazy.hold(work);
                None
            }
            SeaOrmExecutor::Savepoint(block) => {
                block.hold(work);
                None
            }
            SeaOrmExecutor::Pool(_) => Some(work),
        }
    }

    /// A lazy transaction or a block's savepoint writes the records on its
    /// transaction, in its turn, and wakes the relay once it committed; a pool
    /// keeps none, so the jobs are filed at once.
    #[cfg(feature = "queue")]
    pub(crate) fn write_outbox(
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
                SeaOrmExecutor::Lazy(lazy) => lazy.hold(wake),
                SeaOrmExecutor::Savepoint(block) => block.hold(wake),
                SeaOrmExecutor::Pool(_) => {}
            }
            Ok(())
        }))
    }

    #[cfg(feature = "queue")]
    pub(crate) fn remove_outbox<'a>(
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
    pub(crate) fn remove_outbox_unique<'a>(
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

    /// A block on the pool is a transaction of its own; inside a lazy
    /// transaction or another block, a savepoint of its own sent on the same
    /// connection.
    pub(crate) fn atomic(&self) -> Option<Arc<dyn AtomicBlock>> {
        let block = match self {
            SeaOrmExecutor::Pool(pool) => {
                AtomicExecutor::own(Arc::new(LazyTransaction::new(pool.clone(), ATOMIC)))
            }
            SeaOrmExecutor::Lazy(lazy) => {
                AtomicExecutor::savepoint(Arc::new(Savepoint::in_boundary(Arc::clone(lazy))))
            }
            SeaOrmExecutor::Savepoint(block) => {
                AtomicExecutor::savepoint(Arc::new(Savepoint::in_block(Arc::clone(block))))
            }
        };
        Some(Arc::new(block))
    }
}

/// Nothing public installs or hands out the handle a unit runs on, so nothing
/// public names it: a service reads the ambient executor through
/// [`Repo`](crate::Repo) (`decisions/visibility-tiers.md`).
///
/// ```compile_fail,E0425
/// fn named(_: nest_rs_seaorm::SeaOrmExecutor) {}
/// ```
///
/// ```compile_fail,E0425
/// fn named(_: nest_rs_seaorm::LazyTransaction) {}
/// ```
///
/// ```compile_fail,E0425
/// fn named(_: nest_rs_seaorm::FinalizeOutcome) {}
/// ```
///
/// ```compile_fail,E0425
/// fn named(_: nest_rs_seaorm::Savepoint) {}
/// ```
///
/// ```compile_fail,E0425
/// fn named(_: nest_rs_seaorm::CommitError) {}
/// ```
///
/// Nor does this crate's tier 2 keep a handle a caller builds around a
/// transaction of its own.
///
/// ```compile_fail,E0599
/// let _ = nest_rs_seaorm::__private::SeaOrmExecutor::Txn;
/// ```
#[cfg(doctest)]
struct NoPublicHandle;

/// A SeaORM executor as the ambient context holds it, and the type its
/// `as_any` hands out: private to this crate, so a service downcasting
/// [`nest_rs_database::current_executor`] finds no handle it could run a
/// statement on outside [`Repo`](crate::Repo).
pub(crate) struct Installed(pub(crate) SeaOrmExecutor);

impl Installed {
    /// The SeaORM executor `handle` holds, when this crate installed it.
    pub(crate) fn of(handle: &dyn nest_rs_database::Executor) -> Option<&SeaOrmExecutor> {
        handle
            .as_any()
            .downcast_ref::<Self>()
            .map(|installed| &installed.0)
    }
}

impl nest_rs_database::Executor for Installed {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn non_transactional(&self) -> Option<Arc<dyn nest_rs_database::Executor>> {
        self.0.non_transactional()
    }

    fn after_commit(&self, work: Deferred) -> Option<Deferred> {
        self.0.after_commit(work)
    }

    fn atomic(&self) -> Option<Arc<dyn AtomicBlock>> {
        self.0.atomic()
    }

    #[cfg(feature = "queue")]
    fn write_outbox(
        &self,
        records: &mut dyn Iterator<Item = nest_rs_database::OutboxRecord>,
    ) -> Option<futures_util::future::BoxFuture<'_, Result<(), nest_rs_database::OutboxError>>>
    {
        self.0.write_outbox(records)
    }

    #[cfg(feature = "queue")]
    fn remove_outbox<'a>(
        &'a self,
        queue: &'a str,
        id: uuid::Uuid,
    ) -> Option<futures_util::future::BoxFuture<'a, Result<bool, nest_rs_database::OutboxError>>>
    {
        self.0.remove_outbox(queue, id)
    }

    #[cfg(feature = "queue")]
    fn remove_outbox_unique<'a>(
        &'a self,
        queue: &'a str,
        key: &'a str,
    ) -> Option<futures_util::future::BoxFuture<'a, Result<bool, nest_rs_database::OutboxError>>>
    {
        self.0.remove_outbox_unique(queue, key)
    }
}

/// The handle installed, the one way a SeaORM executor built outside this crate
/// reaches the port: a framework suite's, through the port's plumbing.
impl From<SeaOrmExecutor> for Arc<dyn nest_rs_database::Executor> {
    fn from(executor: SeaOrmExecutor) -> Self {
        Arc::new(Installed(executor))
    }
}

/// The SeaORM executor installed in the ambient context for this unit, or
/// `None` outside any. A downcast miss (another ORM's handle) logs an error and
/// surfaces as `None`.
pub fn current_executor() -> Option<SeaOrmExecutor> {
    let dynamic = nest_rs_database::current_executor()?;
    match Installed::of(&*dynamic) {
        Some(executor) => Some(executor.clone()),
        None => {
            tracing::error!(
                target: crate::target::ORM,
                reason = "executor_downcast_miss",
                "ambient executor is not a SeaORM executor"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_database::__private::{with_executor, with_job_executor, with_request_executor};
    use nest_rs_database::{ExecutorScope, current_executor_scope};

    use super::*;

    fn pool() -> SeaOrmExecutor {
        SeaOrmExecutor::Pool(DatabaseConnection::default())
    }

    fn as_seaorm(executor: &Arc<dyn nest_rs_database::Executor>) -> &SeaOrmExecutor {
        Installed::of(&**executor).expect("the handle is a SeaORM executor")
    }

    #[test]
    fn an_unopened_lazy_transaction_hands_out_its_pool() {
        let lazy = SeaOrmExecutor::Lazy(Arc::new(LazyTransaction::new(
            DatabaseConnection::default(),
            "test",
        )));
        let handle = lazy
            .non_transactional()
            .expect("an unopened lazy transaction can step out");
        assert!(matches!(as_seaorm(&handle), SeaOrmExecutor::Pool(_)));
    }

    #[test]
    fn a_pool_has_nothing_to_step_out_of() {
        assert!(pool().non_transactional().is_none());
    }

    #[tokio::test]
    async fn no_ambient_executor_outside_a_scope() {
        assert!(current_executor().is_none());
        assert!(current_executor_scope().is_none());
    }

    #[tokio::test]
    async fn with_executor_installs_the_value_but_no_scope() {
        with_executor(pool().into(), async {
            assert!(matches!(current_executor(), Some(SeaOrmExecutor::Pool(_))));
            assert!(current_executor_scope().is_none());
        })
        .await;
    }

    #[tokio::test]
    async fn with_request_executor_tags_the_scope_as_request() {
        with_request_executor(pool().into(), async {
            assert_eq!(current_executor_scope(), Some(ExecutorScope::Request));
        })
        .await;
    }

    #[tokio::test]
    async fn with_job_executor_tags_the_scope_as_job() {
        with_job_executor(pool().into(), async {
            assert_eq!(current_executor_scope(), Some(ExecutorScope::Job));
        })
        .await;
    }

    #[tokio::test]
    async fn executor_unwinds_when_the_scope_ends() {
        with_request_executor(pool().into(), async {
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
        with_executor(pool().into(), async {
            assert!(current_executor().is_some());
            assert!(current_executor_scope().is_none());
        })
        .await;
        assert!(current_executor().is_none());
        assert!(current_executor_scope().is_none());
    }

    #[tokio::test]
    async fn current_executor_returns_a_fresh_clone_per_call() {
        with_request_executor(pool().into(), async {
            let a = current_executor().expect("installed");
            let b = current_executor().expect("still installed");
            assert!(matches!(a, SeaOrmExecutor::Pool(_)));
            assert!(matches!(b, SeaOrmExecutor::Pool(_)));
        })
        .await;
    }

    #[tokio::test]
    async fn nested_scope_shadows_then_restores_the_outer_scope() {
        with_job_executor(pool().into(), async {
            assert_eq!(current_executor_scope(), Some(ExecutorScope::Job));
            with_request_executor(pool().into(), async {
                assert_eq!(
                    current_executor_scope(),
                    Some(ExecutorScope::Request),
                    "the inner scope wins",
                );
            })
            .await;
            assert_eq!(
                current_executor_scope(),
                Some(ExecutorScope::Job),
                "the outer scope must be restored",
            );
        })
        .await;
    }

    #[test]
    fn executor_clone_preserves_the_pool_variant() {
        let p = pool();
        let cloned = p.clone();
        assert!(matches!(p, SeaOrmExecutor::Pool(_)));
        assert!(matches!(cloned, SeaOrmExecutor::Pool(_)));
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

    use nest_rs_database::__private::with_request_executor;
    use nest_rs_database::Executor as DynExecutor;

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
            "ambient executor is not a SeaORM executor",
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

    use nest_rs_authz::__private::install_ability;
    use nest_rs_authz::{AbilityBuilder, current_ability};
    use nest_rs_database::__private::{with_job_executor, with_request_executor};
    use nest_rs_database::{ExecutorScope, current_executor_scope};
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
                on_pool: matches!(current_executor(), Some(SeaOrmExecutor::Pool(_))),
                scope: current_executor_scope(),
                ability: current_ability(),
            });
        };

        with_request_executor(
            SeaOrmExecutor::Lazy(lazy.clone()).into(),
            install_ability(ability.clone(), nest_rs_database::after_commit(work)),
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
            Installed::of(handle),
            Some(SeaOrmExecutor::Pool(_))
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
        let stepped = SeaOrmExecutor::Lazy(keeping())
            .non_transactional()
            .expect("an unopened boundary steps out");
        keeps_the_outbox_for_a_block_and_a_cancel(&*stepped);

        let lazy = keeping();
        let (ran, set) = flag();
        with_job_executor(
            SeaOrmExecutor::Lazy(lazy.clone()).into(),
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

        let caller = SeaOrmExecutor::Pool(DatabaseConnection::default());
        assert!(caller.remove_outbox("notify", uuid::Uuid::nil()).is_none());
        let block = caller.atomic().expect("a block of its own");
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
            SeaOrmExecutor::Lazy(lazy.clone()).into(),
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
        let escaped = SeaOrmExecutor::Lazy(lazy.clone());
        let (before, held) = flag();
        with_request_executor(escaped.clone().into(), nest_rs_database::after_commit(held)).await;

        assert!(matches!(
            lazy.finalize(true).await,
            FinalizeOutcome::Escaped
        ));
        let (after, late) = flag();
        assert!(
            escaped.after_commit(Box::pin(late)).is_none(),
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
        with_request_executor(SeaOrmExecutor::Lazy(lazy.clone()).into(), async {
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
            SeaOrmExecutor::Lazy(lazy.clone()).into(),
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

    /// A body cut mid-statement leaves it running; the rollback then meets
    /// the answer Postgres gives it at its bound, and the transaction it ends
    /// is rolled back whatever that answer says.
    #[test]
    fn a_rollback_met_by_a_statement_postgres_cancelled_is_said_at_warn_on_every_path() {
        let logs = LogCapture::install();
        for (transport, why) in [("ws", Settled::Failed), ("http", Settled::Poisoned)] {
            report_rollback_failed(transport, &crate::error::tests::cancelled_statement(), why);
        }
        let said = logs.find(
            crate::target::ORM,
            "transaction rolled back after a statement Postgres cancelled; nothing it wrote \
             stands",
        );
        assert_eq!(said.len(), 2, "{:?}", logs.events());
        for line in &said {
            assert_eq!(line.level, "warn");
            assert_eq!(line.field("outcome").as_deref(), Some("rollback"));
            assert!(
                line.field("error")
                    .is_some_and(|error| error.contains("57014")),
                "{line:?}"
            );
        }
        logs.expect_none(crate::target::ORM, "transaction rollback failed");
        logs.expect_none(crate::target::ORM, "poisoned transaction rollback failed");
    }

    #[test]
    fn a_rollback_failing_on_anything_else_is_said_at_error() {
        let logs = LogCapture::install();
        report_rollback_failed(
            "queue",
            &DbErr::Custom("connection reset".into()),
            Settled::Failed,
        );
        let line = logs.expect_one(crate::target::ORM, "transaction rollback failed");
        assert_eq!(line.level, "error");
    }

    #[test]
    fn a_pool_hands_the_work_back_to_run_at_once() {
        let pool = SeaOrmExecutor::Pool(DatabaseConnection::default());
        assert!(pool.after_commit(Box::pin(async {})).is_some());
    }
}
