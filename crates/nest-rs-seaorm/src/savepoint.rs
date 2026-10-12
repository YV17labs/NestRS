use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use nest_rs_database::{AtomicError, Deferred};
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, Statement};

use crate::executor::{
    LazyTransaction, POISON_CLEAN, POISON_TRANSIENT, adopt_into, held_for_commit, record_failure,
    report_discarded, still_held, take_held,
};
use crate::turn::Turn;

/// What a statement on a block that already settled is refused with.
const OUTLIVED: &str = "the atomic block this executor belongs to has settled; nothing more \
     runs on it";

/// The savepoint an [`atomic`](fn@nest_rs_database::atomic) block opened inside a
/// [`LazyTransaction`] or another block's savepoint.
///
/// Lazy like the transaction it sits in: its `SAVEPOINT` goes out with the
/// block's first statement, on the same connection, and from then on the
/// enclosing unit waits for the block to settle — `RELEASE` when it kept its
/// writes, `ROLLBACK TO` otherwise. Its statements poison the block alone; its
/// after-commit work goes to the enclosing unit on release and is dropped on
/// rollback.
pub struct Savepoint {
    enclosing: Enclosing,
    /// How deep the block sits; its savepoint is `nestrs_atomic_<depth>`, which
    /// no other open block shares, since each unit holds one block at a time.
    depth: usize,
    opened: tokio::sync::OnceCell<()>,
    /// Whether the block keeps its enclosing unit's turn.
    holds_turn: AtomicBool,
    settled: AtomicBool,
    poisoned: AtomicU8,
    after_commit: Mutex<Option<Vec<Deferred>>>,
    /// Held by a block nested in this one, from its `SAVEPOINT` until it settles.
    turn: Turn,
}

/// The unit an atomic block sits in.
pub(crate) enum Enclosing {
    Boundary(Arc<LazyTransaction>),
    Block(Arc<Savepoint>),
}

impl Savepoint {
    /// A block inside a lazy transaction.
    pub(crate) fn in_boundary(boundary: Arc<LazyTransaction>) -> Self {
        Self::within(Enclosing::Boundary(boundary), 1)
    }

    /// A block nested in another block.
    pub(crate) fn in_block(block: Arc<Savepoint>) -> Self {
        let depth = block.depth + 1;
        Self::within(Enclosing::Block(block), depth)
    }

    fn within(enclosing: Enclosing, depth: usize) -> Self {
        Self {
            enclosing,
            depth,
            opened: tokio::sync::OnceCell::new(),
            holds_turn: AtomicBool::new(false),
            settled: AtomicBool::new(false),
            poisoned: AtomicU8::new(POISON_CLEAN),
            after_commit: Mutex::new(Some(Vec::new())),
            turn: Turn::new(),
        }
    }

    /// The lazy transaction every savepoint of this stack is sent on.
    pub(crate) fn root(&self) -> &LazyTransaction {
        match &self.enclosing {
            Enclosing::Boundary(boundary) => boundary,
            Enclosing::Block(block) => block.root(),
        }
    }

    pub(crate) fn backend(&self) -> DbBackend {
        self.root().backend()
    }

    fn transport(&self) -> &'static str {
        self.root().transport()
    }

    /// This block's address, then that of every block it sits in.
    pub(crate) fn lineage(&self) -> impl Iterator<Item = usize> + '_ {
        std::iter::successors(Some(self), |block| match &block.enclosing {
            Enclosing::Block(enclosing) => Some(&**enclosing),
            Enclosing::Boundary(_) => None,
        })
        .map(Self::address)
    }

    fn address(&self) -> usize {
        std::ptr::from_ref(self).addr()
    }

    /// Keep `work` until the block settles, captured here for the commit that
    /// will run it.
    pub(crate) fn hold(&self, work: Deferred) {
        self.adopt(vec![held_for_commit(work, self.root().pool())]);
    }

    fn adopt(&self, work: Vec<Deferred>) {
        adopt_into(&self.after_commit, self.transport(), work);
    }

    /// Run one statement inside the block, in its turn.
    pub(crate) async fn run<'a, T, F, Fut>(&'a self, statement: F) -> Result<T, DbErr>
    where
        F: FnOnce(&'a DatabaseTransaction) -> Fut,
        Fut: Future<Output = Result<T, DbErr>> + 'a,
    {
        let _turn = self.turn.for_statement().await?;
        self.run_unturned(statement).await
    }

    /// Run one statement inside the block, poisoning the block on either
    /// failure, the `SAVEPOINT` its first statement sends included.
    async fn run_unturned<'a, T, F, Fut>(&'a self, statement: F) -> Result<T, DbErr>
    where
        F: FnOnce(&'a DatabaseTransaction) -> Fut,
        Fut: Future<Output = Result<T, DbErr>> + 'a,
    {
        record_failure(
            &self.poisoned,
            match self.open().await {
                Ok(txn) => statement(txn).await,
                Err(err) => Err(err),
            },
        )
    }

    /// The transaction the block's statements run on, its `SAVEPOINT` sent
    /// once first.
    async fn open(&self) -> Result<&DatabaseTransaction, DbErr> {
        if self.settled.load(Ordering::Acquire) {
            return Err(DbErr::Custom(OUTLIVED.to_owned()));
        }
        self.opened
            .get_or_try_init(|| self.send_savepoint())
            .await?;
        self.root()
            .opened()
            .ok_or_else(|| DbErr::Custom(OUTLIVED.to_owned()))
    }

    /// Take the enclosing unit's turn for as long as the block is open, and
    /// send its `SAVEPOINT` there: a refused one poisons the enclosing unit,
    /// whose transaction Postgres has just aborted.
    async fn send_savepoint(&self) -> Result<(), DbErr> {
        if !self.holds_turn.load(Ordering::Acquire) {
            self.enclosing.turn().for_block(self.address()).await?;
            self.holds_turn.store(true, Ordering::Release);
        }
        let statement = self.statement(format!("SAVEPOINT {}", self.name()));
        let mut unsettled = Unsettled::armed(&self.enclosing, self.transport());
        let sent = self
            .enclosing
            .run_unturned(|txn| txn.execute_raw(statement))
            .await;
        unsettled.disarm();
        if sent.is_err() {
            self.give_back_turn();
        }
        sent.map(drop)
    }

    fn name(&self) -> String {
        format!("nestrs_atomic_{}", self.depth)
    }

    fn statement(&self, sql: String) -> Statement {
        Statement::from_string(self.backend(), sql)
    }

    fn give_back_turn(&self) {
        if self.holds_turn.swap(false, Ordering::AcqRel) {
            self.enclosing.turn().give_back();
        }
    }

    /// Release the savepoint and hand the held work to the enclosing unit when
    /// `ok` and nothing failed inside, or roll it back and drop that work.
    pub(crate) async fn settle(self: Arc<Self>, ok: bool) -> Result<(), AtomicError> {
        let transport = self.transport();
        // An escaped handle's statement in flight finishes first; none starts after.
        let own_turn = self.turn.for_statement().await;
        self.settled.store(true, Ordering::Release);
        let held = take_held(&self.after_commit);
        let escaped = Arc::strong_count(&self) > 1;
        let poisoned = self.poisoned.load(Ordering::Relaxed);
        let keep = ok && !escaped && poisoned == POISON_CLEAN;

        if self.opened.get().is_some() {
            let name = self.name();
            let sql = if keep {
                format!("RELEASE SAVEPOINT {name}")
            } else {
                format!("ROLLBACK TO SAVEPOINT {name}; RELEASE SAVEPOINT {name}")
            };
            let mut unsettled = Unsettled::armed(&self.enclosing, transport);
            let sent = match self.root().opened() {
                Some(txn) => txn.execute_unprepared(&sql).await,
                None => Err(DbErr::Custom(OUTLIVED.to_owned())),
            };
            unsettled.disarm();
            self.give_back_turn();
            if let Err(err) = sent {
                self.enclosing.poison_deterministic();
                report_discarded(transport, held.len());
                tracing::error!(
                    target: crate::target::ORM,
                    transport,
                    error = %nest_rs_core::error_message(&err),
                    outcome = "rollback_and_fail",
                    "an atomic block's savepoint could not be settled; its unit cannot commit",
                );
                return Err(AtomicError::Database(Box::new(err)));
            }
        }
        self.give_back_turn();
        drop(own_turn);

        if escaped {
            report_discarded(transport, held.len());
            tracing::error!(
                target: crate::target::ORM,
                transport,
                outcome = "rollback",
                "atomic block's executor escaped into a task that outlived it",
            );
            return Err(AtomicError::Escaped);
        }
        if keep {
            self.enclosing.adopt(held);
            return Ok(());
        }
        report_discarded(transport, held.len());
        match (ok, poisoned) {
            (true, POISON_CLEAN) | (false, _) => Ok(()),
            (true, verdict) => Err(AtomicError::Poisoned {
                retryable: verdict == POISON_TRANSIENT,
            }),
        }
    }
}

impl Drop for Savepoint {
    /// A block dropped before it settled — its future cancelled — leaves its
    /// savepoint open on the connection: nothing can vouch for what its unit
    /// writes next, so that unit no longer commits.
    fn drop(&mut self) {
        let transport = self.transport();
        report_discarded(transport, still_held(&mut self.after_commit));
        if !self.settled.load(Ordering::Acquire) && self.opened.get().is_some() {
            self.enclosing.poison_deterministic();
            report_abandoned(transport);
        }
        self.give_back_turn();
    }
}

impl Enclosing {
    fn turn(&self) -> &Turn {
        match self {
            Enclosing::Boundary(boundary) => boundary.turn(),
            Enclosing::Block(block) => &block.turn,
        }
    }

    fn poison_deterministic(&self) {
        match self {
            Enclosing::Boundary(boundary) => boundary.poison_deterministic(),
            Enclosing::Block(block) => {
                crate::executor::poison_once(&block.poisoned, crate::executor::POISON_DETERMINISTIC)
            }
        }
    }

    fn adopt(&self, work: Vec<Deferred>) {
        match self {
            Enclosing::Boundary(boundary) => boundary.adopt(work),
            Enclosing::Block(block) => block.adopt(work),
        }
    }

    /// Run a statement of the enclosing unit's while the block nested in it
    /// holds that unit's turn: its own `SAVEPOINT`.
    async fn run_unturned<'a, T, F, Fut>(&'a self, statement: F) -> Result<T, DbErr>
    where
        F: FnOnce(&'a DatabaseTransaction) -> Fut,
        Fut: Future<Output = Result<T, DbErr>> + 'a,
    {
        match self {
            Enclosing::Boundary(boundary) => boundary.run_unturned(statement).await,
            Enclosing::Block(block) => Box::pin(block.run_unturned(statement)).await,
        }
    }
}

/// Poisons the enclosing unit when the future sending a block's `SAVEPOINT`,
/// `RELEASE` or `ROLLBACK TO` is dropped before the database answered: the
/// savepoint is then in a state nothing can vouch for.
struct Unsettled<'a> {
    enclosing: &'a Enclosing,
    transport: &'static str,
    armed: bool,
}

impl<'a> Unsettled<'a> {
    fn armed(enclosing: &'a Enclosing, transport: &'static str) -> Self {
        Self {
            enclosing,
            transport,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Unsettled<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.enclosing.poison_deterministic();
            report_abandoned(self.transport);
        }
    }
}

fn report_abandoned(transport: &'static str) {
    tracing::warn!(
        target: crate::target::ORM,
        transport,
        outcome = "abandoned",
        "atomic block abandoned before it settled; its unit cannot commit",
    );
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use nest_rs_database::{AtomicError, after_commit, atomic};
    use sea_orm::DatabaseConnection;

    use crate::executor::{
        Executor, FinalizeOutcome, LazyTransaction, current_executor, with_request_executor,
    };

    #[derive(Debug)]
    enum Failure {
        Atomic(AtomicError),
        Refused,
    }

    impl From<AtomicError> for Failure {
        fn from(err: AtomicError) -> Self {
            Self::Atomic(err)
        }
    }

    /// A unit on a pool that answers no statement: one reaching it would fail.
    fn unreachable_unit() -> Arc<LazyTransaction> {
        Arc::new(LazyTransaction::new(DatabaseConnection::default(), "test"))
    }

    fn flag() -> (Arc<AtomicBool>, impl Future<Output = ()> + Send + 'static) {
        let flag = Arc::new(AtomicBool::new(false));
        let set = Arc::clone(&flag);
        (flag, async move { set.store(true, Ordering::SeqCst) })
    }

    #[tokio::test]
    async fn a_block_that_never_queries_sends_nothing() {
        let unit = unreachable_unit();
        let outcome = with_request_executor(
            Executor::Lazy(Arc::clone(&unit)),
            atomic(async {
                Ok::<_, Failure>(matches!(current_executor(), Some(Executor::Savepoint(_))))
            }),
        )
        .await;

        assert!(matches!(outcome, Ok(true)), "{outcome:?}");
        assert!(!unit.is_opened(), "no BEGIN, no SAVEPOINT");
        assert!(matches!(
            unit.finalize(true).await,
            FinalizeOutcome::NoTransaction
        ));
    }

    #[tokio::test]
    async fn a_block_on_the_pool_that_never_queries_opens_nothing() {
        let (ran, work) = flag();
        let outcome = with_request_executor(
            Executor::Pool(DatabaseConnection::default()),
            atomic(async {
                after_commit(work).await;
                Ok::<_, Failure>(matches!(current_executor(), Some(Executor::Lazy(_))))
            }),
        )
        .await;

        assert!(matches!(outcome, Ok(true)), "{outcome:?}");
        assert!(
            ran.load(Ordering::SeqCst),
            "a block of its own commits on its own: its work ran as it settled"
        );
    }

    #[tokio::test]
    async fn released_work_waits_for_the_units_commit() {
        let unit = unreachable_unit();
        let (ran, work) = flag();
        let outcome = with_request_executor(
            Executor::Lazy(Arc::clone(&unit)),
            atomic(async {
                after_commit(work).await;
                Ok::<_, Failure>(())
            }),
        )
        .await;
        assert!(outcome.is_ok(), "{outcome:?}");
        assert!(
            !ran.load(Ordering::SeqCst),
            "the unit has not committed yet"
        );

        assert!(matches!(
            unit.finalize(true).await,
            FinalizeOutcome::NoTransaction
        ));
        assert!(ran.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn work_of_a_block_that_rolled_back_never_runs() {
        let unit = unreachable_unit();
        let (ran, work) = flag();
        let outcome = with_request_executor(
            Executor::Lazy(Arc::clone(&unit)),
            atomic(async {
                after_commit(work).await;
                Err::<(), _>(Failure::Refused)
            }),
        )
        .await;
        assert!(matches!(outcome, Err(Failure::Refused)), "{outcome:?}");

        assert!(matches!(
            unit.finalize(true).await,
            FinalizeOutcome::NoTransaction
        ));
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_nested_blocks_work_waits_for_the_outermost_commit_and_dies_with_its_enclosing_block()
    {
        let unit = unreachable_unit();
        let (kept, kept_work) = flag();
        let (dropped, dropped_work) = flag();
        with_request_executor(Executor::Lazy(Arc::clone(&unit)), async {
            let released = atomic(async {
                atomic(async {
                    after_commit(kept_work).await;
                    Ok::<_, Failure>(())
                })
                .await
            })
            .await;
            assert!(released.is_ok(), "{released:?}");
            let rolled_back = atomic(async {
                let inner = atomic(async {
                    after_commit(dropped_work).await;
                    Ok::<_, Failure>(())
                })
                .await;
                assert!(inner.is_ok(), "{inner:?}");
                Err::<(), _>(Failure::Refused)
            })
            .await;
            assert!(matches!(rolled_back, Err(Failure::Refused)));
        })
        .await;
        assert!(!kept.load(Ordering::SeqCst));

        assert!(matches!(
            unit.finalize(true).await,
            FinalizeOutcome::NoTransaction
        ));
        assert!(kept.load(Ordering::SeqCst));
        assert!(!dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_unit_that_rolls_back_drops_what_its_blocks_released() {
        let unit = unreachable_unit();
        let (ran, work) = flag();
        let released = with_request_executor(
            Executor::Lazy(Arc::clone(&unit)),
            atomic(async {
                after_commit(work).await;
                Ok::<_, Failure>(())
            }),
        )
        .await;
        assert!(released.is_ok(), "{released:?}");

        assert!(matches!(
            unit.finalize(false).await,
            FinalizeOutcome::NoTransaction
        ));
        assert!(!ran.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_block_whose_executor_escapes_answers_so_even_unopened() {
        let unit = unreachable_unit();
        let escaped = std::sync::Mutex::new(None);
        let outcome = with_request_executor(
            Executor::Lazy(Arc::clone(&unit)),
            atomic(async {
                *escaped
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = current_executor();
                Ok::<_, Failure>(())
            }),
        )
        .await;

        assert!(
            matches!(outcome, Err(Failure::Atomic(AtomicError::Escaped))),
            "{outcome:?}"
        );
    }
}
