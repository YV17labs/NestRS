//! A unit's events name its edge (`http`, `ws`, `queue`, …) in `transport`, as
//! every other framework event does; a unit no edge dispatches is named in full.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use nest_rs_core::{Continuation, TaskContext, Unanswered, within};
use nest_rs_database::{
    AtomicBlock, ExecutorScope, OpenUnit, Settlement, TransactionPolicy, Unhonoured, UnitOfWork,
    UnitSpec,
};
use sea_orm::DatabaseConnection;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::task::TaskTracker;

use crate::SeaOrmConfig;
use crate::atomic::{AtomicExecutor, PoolCommit};
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
/// A transaction's settle runs on a task of its own, tracked here, so a
/// dropped edge future never cuts its `COMMIT`: it answers within the settle
/// net, its statement budget plus a second, and in doubt past it, and its
/// after-commit work is waited for until the unit's deadline.
///
/// Bound as `Arc<dyn UnitOfWork>` by
/// [`SeaOrmDatabaseModule`](crate::SeaOrmDatabaseModule), so every edge opens
/// its units through it.
pub struct SeaOrmUnitOfWork {
    db: DatabaseConnection,
    /// The handle every pool unit installs, shared.
    pool: Arc<Executor>,
    observe_conflicts: bool,
    settle_net: Duration,
    settles: Settles,
    /// The outbox a unit's transaction writes a push's jobs into.
    #[cfg(feature = "queue")]
    outbox: Option<Arc<crate::queue::SeaOrmOutbox>>,
}

impl SeaOrmUnitOfWork {
    /// The unit of work over `db`, reading how to report a commit-time conflict
    /// and how long a statement runs from `config`.
    ///
    /// Its settles run on tasks nothing drains when the app ends; the one
    /// [`SeaOrmDatabaseModule`](crate::SeaOrmDatabaseModule) binds is drained
    /// before the pool closes.
    pub fn new(db: Arc<DatabaseConnection>, config: &SeaOrmConfig) -> Self {
        Self::draining_into(db, config, Settles::default())
    }

    /// [`new`](Self::new), its settles tracked by `settles`.
    pub(crate) fn draining_into(
        db: Arc<DatabaseConnection>,
        config: &SeaOrmConfig,
        settles: Settles,
    ) -> Self {
        let db = (*db).clone();
        Self {
            pool: Arc::new(Executor::Pool(db.clone())),
            db,
            observe_conflicts: config.observe_serialization_conflicts,
            settle_net: settle_net(config),
            settles,
            #[cfg(feature = "queue")]
            outbox: None,
        }
    }

    /// The same unit of work, its transactions writing a push's jobs into
    /// `outbox` once a relay armed it.
    #[cfg(feature = "queue")]
    pub(crate) fn keeping(mut self, outbox: Option<Arc<crate::queue::SeaOrmOutbox>>) -> Self {
        self.outbox = outbox;
        self
    }

    /// A transaction opened on its first touch, for `transport`.
    fn lazy(&self, transport: &'static str) -> LazyTransaction {
        let lazy = LazyTransaction::new(self.db.clone(), transport);
        #[cfg(feature = "queue")]
        let lazy = lazy.keeping(self.outbox.clone());
        lazy
    }
}

/// The settle net: what Postgres holds a statement to, `COMMIT` included,
/// plus a second, so it fires only on a database that stopped bounding itself.
fn settle_net(config: &SeaOrmConfig) -> Duration {
    config.statement_timeout() + Duration::from_secs(1)
}

impl UnitOfWork for SeaOrmUnitOfWork {
    fn open(&self, spec: &UnitSpec) -> Result<Box<dyn OpenUnit>, Unhonoured> {
        let transport = spec
            .unit
            .edge()
            .map_or(spec.unit.name(), nest_rs_core::Edge::as_str);
        match spec.policy {
            TransactionPolicy::Pool => Ok(Box::new(OnPool {
                unit: Arc::new(PoolUnit {
                    pool: Arc::clone(&self.pool),
                    settles: self.settles.clone(),
                    unseen: OnceLock::new(),
                    #[cfg(feature = "queue")]
                    outbox: self.outbox.clone(),
                }),
                transport,
            })),
            TransactionPolicy::PerUnit => Ok(Box::new(InTransaction {
                lazy: Arc::new(self.lazy(transport)),
                observe_conflicts: self.observe_conflicts && spec.scope == ExecutorScope::Request,
                net: self.settle_net,
                deadline: spec.deadline,
                settles: self.settles.clone(),
            })),
            // A policy this driver does not know is refused before the body runs.
            _ => Err(Unhonoured::deterministic(UNSUPPORTED_POLICY)),
        }
    }

    fn settle_net(&self) -> Duration {
        self.settle_net
    }
}

/// The tasks a unit of work's settles run on, drained before the pool closes.
#[derive(Clone, Default)]
pub(crate) struct Settles(TaskTracker);

impl Settles {
    /// Run `task` as a settle the drain waits for.
    pub(crate) fn spawn<F>(&self, task: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.0.spawn(task)
    }

    /// Wait for every settle still running; one started after is run as well.
    pub(crate) async fn drain(&self) {
        self.0.close();
        self.0.wait().await;
        self.0.reopen();
    }
}

/// A unit on the pool: each statement commits on its own, and nothing settles
/// but a failure cut while an atomic block's own commit ran.
struct OnPool {
    unit: Arc<PoolUnit>,
    transport: &'static str,
}

impl OpenUnit for OnPool {
    fn executor(&self) -> Arc<dyn nest_rs_database::Executor> {
        Arc::clone(&self.unit) as Arc<dyn nest_rs_database::Executor>
    }

    fn settle(self: Box<Self>, ok: bool) -> Pin<Box<dyn Future<Output = Settlement> + Send>> {
        if !ok && self.unit.cut_mid_commit() {
            tracing::error!(
                target: crate::target::ORM,
                transport = self.transport,
                outcome = "in_doubt",
                "the unit was cut while an atomic block's commit ran; its writes may have landed",
            );
            return Box::pin(std::future::ready(Settlement::Unhonoured(
                Unhonoured::IN_DOUBT,
            )));
        }
        Box::pin(Settled)
    }
}

/// The handle a pool unit installs: the pool, on which an atomic block is a
/// transaction of its own committed on the binding's settle task.
pub(crate) struct PoolUnit {
    pool: Arc<Executor>,
    settles: Settles,
    /// The commits of the blocks the body opened whose answer it has not seen;
    /// allocated by the first block.
    unseen: OnceLock<Arc<AtomicUsize>>,
    /// The outbox a block of its own writes a push's jobs into; a push on the
    /// pool itself is filed at once.
    #[cfg(feature = "queue")]
    outbox: Option<Arc<crate::queue::SeaOrmOutbox>>,
}

impl PoolUnit {
    /// Whether the body ended before seeing a block's commit answer.
    fn cut_mid_commit(&self) -> bool {
        self.unseen
            .get()
            .is_some_and(|unseen| unseen.load(Ordering::Relaxed) > 0)
    }

    /// The outbox a cancel on the pool deletes a kept record from: the
    /// binding's, armed by a relay.
    #[cfg(feature = "queue")]
    fn outbox(&self) -> Option<&Arc<crate::queue::SeaOrmOutbox>> {
        self.outbox.as_ref().filter(|outbox| outbox.is_armed())
    }
}

impl nest_rs_database::Executor for PoolUnit {
    fn as_any(&self) -> &dyn Any {
        &*self.pool
    }

    fn atomic(&self) -> Option<Arc<dyn AtomicBlock>> {
        let own = self.pool.own_block()?;
        #[cfg(feature = "queue")]
        let own = own.keeping(self.outbox.clone());
        let unseen = Arc::clone(self.unseen.get_or_init(Arc::default));
        Some(Arc::new(AtomicExecutor::own_on_pool(
            Arc::new(own),
            PoolCommit::new(self.settles.clone(), unseen),
        )))
    }

    /// A record committed by an earlier unit and not yet filed is deleted on
    /// the pool, outside every transaction.
    #[cfg(feature = "queue")]
    fn remove_outbox<'a>(
        &'a self,
        queue: &'a str,
        id: uuid::Uuid,
    ) -> Option<futures_util::future::BoxFuture<'a, Result<bool, nest_rs_database::OutboxError>>>
    {
        let outbox = self.outbox()?;
        Some(Box::pin(
            outbox.remove_on_pool(crate::queue::removal_of_job(queue, id)),
        ))
    }

    #[cfg(feature = "queue")]
    fn remove_outbox_unique<'a>(
        &'a self,
        queue: &'a str,
        key: &'a str,
    ) -> Option<futures_util::future::BoxFuture<'a, Result<bool, nest_rs_database::OutboxError>>>
    {
        let outbox = self.outbox()?;
        Some(Box::pin(
            outbox.remove_on_pool(crate::queue::removal_of_key(queue, key)),
        ))
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
    net: Duration,
    deadline: Option<Instant>,
    settles: Settles,
}

impl OpenUnit for InTransaction {
    fn executor(&self) -> Arc<dyn nest_rs_database::Executor> {
        Arc::new(Executor::Lazy(Arc::clone(&self.lazy)))
    }

    fn settle(self: Box<Self>, ok: bool) -> Pin<Box<dyn Future<Output = Settlement> + Send>> {
        let Self {
            lazy,
            observe_conflicts,
            net,
            deadline,
            settles,
        } = *self;
        let transport = lazy.transport();
        // Nothing to send and nothing to run: settled here, without a task.
        if !lazy.is_opened() && !lazy.holds_work() {
            return Box::pin(async move {
                let (outcome, _) = lazy.answer(ok).await;
                classify(outcome, ok, transport, observe_conflicts)
            });
        }
        let (answer, answered) = oneshot::channel();
        // The correlation crosses with it: the events it files name the unit,
        // and a task without it would have them dropped at teardown unsaid.
        let task = TaskContext::capture(Continuation::Commit).carry(async move {
            let settled = nest_rs_core::panic::contain(async move {
                let (outcome, held) = lazy.answer(ok).await;
                // The edge may have stopped listening; the settle goes on regardless.
                let _unheard = answer.send(classify(outcome, ok, transport, observe_conflicts));
                held.run().await;
            })
            .await;
            if let Err(payload) = settled {
                nest_rs_core::contained_panic!(
                    target: crate::target::ORM,
                    payload.as_ref(),
                    "a unit's settle panicked",
                    transport,
                );
            }
        });
        let finished = settles.spawn(task);
        Box::pin(told(ok, transport, net, deadline, answered, async move {
            drop(finished.await);
        }))
    }
}

/// What a settle running on its task tells the edge: the transaction's answer
/// within `net` — in doubt past it for a commit — then the end of the work it
/// held for the commit, waited for until the unit's `deadline`.
async fn told(
    ok: bool,
    transport: &'static str,
    net: Duration,
    deadline: Option<Instant>,
    answered: impl Future<Output = Result<Settlement, oneshot::error::RecvError>>,
    finished: impl Future<Output = ()>,
) -> Settlement {
    let settlement = match within(net, answered).await {
        Ok(Ok(settlement)) => settlement,
        // The task unwound before answering, and said so.
        Ok(Err(_)) | Err(Unanswered::Panicked(_)) => return unanswered(ok),
        Err(_) => {
            let net_ms = u64::try_from(net.as_millis()).unwrap_or(u64::MAX);
            if ok {
                tracing::error!(
                    target: crate::target::ORM,
                    transport,
                    outcome = "in_doubt",
                    net_ms,
                    "the unit's commit did not answer within its net; its writes may have landed",
                );
            } else {
                tracing::warn!(
                    target: crate::target::ORM,
                    transport,
                    net_ms,
                    "the unit's rollback did not answer within its net; it goes on",
                );
            }
            return unanswered(ok);
        }
    };
    match deadline {
        Some(deadline) => {
            if tokio::time::timeout_at(deadline, finished).await.is_err() {
                tracing::debug!(
                    target: crate::target::ORM,
                    transport,
                    "after-commit work outlived the unit's deadline",
                );
            }
        }
        None => finished.await,
    }
    settlement
}

/// What a settle that never answered means: a commit that may have landed, or
/// a rollback, whose failure stands.
fn unanswered(ok: bool) -> Settlement {
    if ok {
        Settlement::Unhonoured(Unhonoured::IN_DOUBT)
    } else {
        Settlement::Settled
    }
}

/// The settlement a finalized boundary means for its unit.
fn classify(
    outcome: FinalizeOutcome,
    ok: bool,
    transport: &'static str,
    observe_conflicts: bool,
) -> Settlement {
    match outcome {
        // Deterministic: the body spawned a task holding the executor, and
        // running it again spawns the same one.
        FinalizeOutcome::Escaped if ok => {
            Settlement::Unhonoured(Unhonoured::deterministic(ESCAPED))
        }
        FinalizeOutcome::NoTransaction
        | FinalizeOutcome::Committed
        | FinalizeOutcome::RolledBack
        | FinalizeOutcome::Escaped => Settlement::Settled,
        // The body swallowed a failed statement and succeeded: nothing it wrote
        // could land.
        FinalizeOutcome::Poisoned { retryable } => {
            Settlement::Unhonoured(Unhonoured::new(POISONED, retryable))
        }
        FinalizeOutcome::CommitFailed(err) => commit_failed(&err, transport, observe_conflicts),
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

    /// A pool unit settles nothing, but a failure its body met before a block
    /// of its own saw its commit answered: that commit may have landed.
    #[tokio::test]
    async fn a_pool_unit_failed_before_a_block_saw_its_commit_answered_is_in_doubt() {
        let logs = nest_rs_testing::LogCapture::install();
        for (blocks_unseen, ok, settled) in [
            (None, false, Settlement::Settled),
            (Some(0), false, Settlement::Settled),
            (Some(1), true, Settlement::Settled),
            (Some(1), false, Settlement::Unhonoured(Unhonoured::IN_DOUBT)),
        ] {
            let unit = Arc::new(PoolUnit {
                pool: Arc::new(Executor::Pool(DatabaseConnection::default())),
                settles: Settles::default(),
                unseen: OnceLock::new(),
                #[cfg(feature = "queue")]
                outbox: None,
            });
            if let Some(unseen) = blocks_unseen {
                let block = nest_rs_database::Executor::atomic(&*unit)
                    .expect("a pool unit offers a block of its own");
                assert!(matches!(seaorm(&(block as _)), Executor::Lazy(_)));
                let count = unit.unseen.get().expect("the block counts its commit");
                count.fetch_add(unseen, Ordering::Relaxed);
            }
            let open = Box::new(OnPool {
                unit,
                transport: "queue",
            });
            assert_eq!(
                open.settle(ok).await,
                settled,
                "{blocks_unseen:?} unseen, ok = {ok}"
            );
        }
        let line = logs.expect_one(
            crate::target::ORM,
            "the unit was cut while an atomic block's commit ran; its writes may have landed",
        );
        assert_eq!(line.field("outcome").as_deref(), Some("in_doubt"));
    }

    #[test]
    fn the_settle_net_is_the_statement_budget_and_a_second() {
        assert_eq!(unit_of_work().settle_net(), Duration::from_secs(16));
        let pinned = SeaOrmConfig {
            statement_timeout_secs: Some(30),
            ..SeaOrmConfig::default()
        };
        assert_eq!(settle_net(&pinned), Duration::from_secs(31));
    }

    const NET: Duration = Duration::from_secs(16);

    /// An answer `after` from now, as the settle's task tells it.
    async fn answering(
        after: Duration,
        settlement: Settlement,
    ) -> Result<Settlement, oneshot::error::RecvError> {
        tokio::time::sleep(after).await;
        Ok(settlement)
    }

    #[tokio::test(start_paused = true)]
    async fn a_commit_that_never_answers_is_in_doubt_at_the_net_and_said_once() {
        let logs = nest_rs_testing::LogCapture::install();
        let started = Instant::now();
        let settled = told(
            true,
            "ws",
            NET,
            Some(started + Duration::from_secs(1)),
            std::future::pending(),
            std::future::pending(),
        )
        .await;
        assert_eq!(settled, Settlement::Unhonoured(Unhonoured::IN_DOUBT));
        assert_eq!(started.elapsed(), NET, "never cut at the deadline");
        let line = logs.expect_one(
            crate::target::ORM,
            "the unit's commit did not answer within its net; its writes may have landed",
        );
        assert_eq!(line.level, "error");
        assert_eq!(line.field("outcome").as_deref(), Some("in_doubt"));
        assert_eq!(line.field("transport").as_deref(), Some("ws"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_rollback_that_never_answers_lets_the_failure_stand() {
        let settled = told(
            false,
            "queue",
            NET,
            None,
            std::future::pending(),
            std::future::pending(),
        )
        .await;
        assert_eq!(settled, Settlement::Settled);
    }

    #[tokio::test(start_paused = true)]
    async fn a_commit_answered_past_the_deadline_is_told_without_the_work_it_held() {
        let started = Instant::now();
        let settled = told(
            true,
            "http",
            NET,
            Some(started + Duration::from_millis(100)),
            answering(Duration::from_millis(2180), Settlement::Settled),
            // An async block: a bare `sleep` would count from here and end before the answer.
            async { tokio::time::sleep(Duration::from_secs(2)).await },
        )
        .await;
        assert_eq!(settled, Settlement::Settled);
        assert_eq!(
            started.elapsed(),
            Duration::from_millis(2180),
            "told with the commit's answer, never after the work it held"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn after_commit_work_is_waited_for_until_the_deadline_then_runs_on() {
        let logs = nest_rs_testing::LogCapture::install();
        let started = Instant::now();
        let settled = told(
            true,
            "queue",
            NET,
            Some(started + Duration::from_secs(1)),
            answering(Duration::from_millis(100), Settlement::Settled),
            tokio::time::sleep(Duration::from_secs(2)),
        )
        .await;
        assert_eq!(
            settled,
            Settlement::Settled,
            "a slow listener never fails a commit"
        );
        assert_eq!(started.elapsed(), Duration::from_secs(1));
        let line = logs.expect_one(
            crate::target::ORM,
            "after-commit work outlived the unit's deadline",
        );
        assert_eq!(line.level, "debug");

        let started = Instant::now();
        let settled = told(
            true,
            "queue",
            NET,
            None,
            answering(Duration::ZERO, Settlement::Settled),
            tokio::time::sleep(Duration::from_secs(2)),
        )
        .await;
        assert_eq!(settled, Settlement::Settled);
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(2),
            "with no deadline, it is waited for whole"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_drain_waits_for_every_settle_still_running() {
        let settles = Settles::default();
        let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&ended);
        drop(settles.0.spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }));
        settles.drain().await;
        assert!(ended.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[cfg(feature = "queue")]
    #[test]
    fn a_units_transaction_keeps_jobs_once_a_relay_armed_its_outbox_and_a_pool_never() {
        let record = || {
            std::iter::once(nest_rs_database::OutboxRecord::new(
                "notify",
                uuid::Uuid::nil(),
                "{}".to_owned(),
                None,
                Duration::ZERO,
            ))
        };
        let unread = || {
            std::iter::from_fn(|| -> Option<nest_rs_database::OutboxRecord> {
                panic!("a handle keeping no outbox builds no record")
            })
        };
        let unarmed = Arc::new(crate::queue::SeaOrmOutbox::new(
            DatabaseConnection::default(),
        ));
        let unit_of_work = unit_of_work().keeping(Some(unarmed));
        let open = unit_of_work
            .open(&UnitSpec::new(
                MESSAGE,
                ExecutorScope::Request,
                TransactionPolicy::PerUnit,
            ))
            .expect("opened");
        assert!(
            open.executor().write_outbox(&mut unread()).is_none(),
            "no relay drains it: filed at once"
        );

        let unit_of_work = unit_of_work.keeping(Some(crate::queue::armed()));
        let open = unit_of_work
            .open(&UnitSpec::new(
                MESSAGE,
                ExecutorScope::Request,
                TransactionPolicy::PerUnit,
            ))
            .expect("opened");
        assert!(open.executor().write_outbox(&mut record()).is_some());
        let open = unit_of_work
            .open(&UnitSpec::new(
                MESSAGE,
                ExecutorScope::Request,
                TransactionPolicy::Pool,
            ))
            .expect("opened");
        assert!(
            open.executor().write_outbox(&mut unread()).is_none(),
            "a pool files at once"
        );
        let caller = Executor::Pool(DatabaseConnection::default());
        assert!(
            nest_rs_database::Executor::write_outbox(&caller, &mut unread()).is_none(),
            "a handle the caller built keeps no outbox"
        );
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
