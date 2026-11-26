use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use nest_rs_core::__private::ExtendedContext;
use nest_rs_core::{AmbientValue, Continuation};

use crate::atomic::AtomicBlock;
use crate::outbox::{Outbox, OutboxKept};
use crate::unit::Bound;

/// Work held until the ambient unit of work commits — what [`after_commit`]
/// hands a [`HeldTransaction`] to keep.
pub type Deferred = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// An ambient handle to a unit of database work, installed in the ambient
/// context for the lifetime of a request or a worker job.
///
/// A driver says one thing of its handle, [what it holds](Self::holds), and
/// the port reads everything else off the answer: the block
/// [`atomic`](fn@crate::atomic) opens, whether [`after_commit`] waits, the
/// savepoint a policy runs an attempt in, where a pushed job is kept. A
/// wrapper around a handle answers what that handle holds.
///
/// ```
/// use std::sync::Arc;
///
/// use nest_rs_database::{
///     AtomicBlock, Deferred, Executor, HeldPool, HeldTransaction, Holds, Outbox, OutboxKept,
/// };
///
/// struct Handle {
///     pool: Pool,
///     transaction: Option<Transaction>,
/// }
/// struct Pool;
/// struct Transaction;
///
/// impl Executor for Handle {
///     fn holds(&self) -> Holds<'_> {
///         match &self.transaction {
///             Some(transaction) => Holds::Transaction(transaction),
///             None => Holds::Pool(&self.pool),
///         }
///     }
/// }
///
/// impl HeldPool for Pool {
///     fn transaction(&self) -> Arc<dyn AtomicBlock> {
///         # unreachable!()
///         // BEGIN on the block's first statement, COMMIT or ROLLBACK as it settles.
///     }
///
///     fn outbox(&self) -> Option<&dyn OutboxKept> {
///         None
///     }
/// }
///
/// impl HeldTransaction for Transaction {
///     fn after_commit(&self, work: Deferred) {
///         # drop(work);
///         // Kept until the transaction's boundary settles.
///     }
///
///     fn savepoint(&self) -> Arc<dyn AtomicBlock> {
///         # unreachable!()
///         // SAVEPOINT on the block's first statement, RELEASE or ROLLBACK TO as it settles.
///     }
///
///     fn outbox(&self) -> Option<&dyn Outbox> {
///         None
///     }
/// }
/// ```
///
/// A handle that does not say what it holds is no executor: the handle above,
/// its `holds` left out.
///
/// ```compile_fail,E0046
/// struct Handle;
///
/// impl nest_rs_database::Executor for Handle {}
/// ```
///
/// Nor is a transaction offering no savepoint one a handle can hold: the
/// transaction above, its `savepoint` left out.
///
/// ```compile_fail,E0046
/// use nest_rs_database::{Deferred, HeldTransaction, Outbox};
///
/// struct Transaction;
///
/// impl HeldTransaction for Transaction {
///     fn after_commit(&self, work: Deferred) {
///         drop(work);
///     }
///
///     fn outbox(&self) -> Option<&dyn Outbox> {
///         None
///     }
/// }
/// ```
pub trait Executor: Send + Sync + 'static {
    /// What this handle runs its statements on: the pool, or a transaction.
    fn holds(&self) -> Holds<'_>;
}

/// What an [`Executor`] runs its statements on, as its driver says it
/// ([`Executor::holds`]): the one fact the port decides by.
#[derive(Clone, Copy)]
#[expect(
    clippy::exhaustive_enums,
    reason = "closed by definition: a statement commits on its own or with a transaction, and every reader decides by naming both"
)]
pub enum Holds<'a> {
    /// The pool: each statement commits on its own.
    Pool(&'a dyn HeldPool),
    /// A transaction — a unit's, the one an atomic block opens of its own on
    /// the pool, or a block's savepoint inside one: what is written there
    /// stands once it commits.
    Transaction(&'a dyn HeldTransaction),
}

impl<'a> Holds<'a> {
    /// What the handle holds, as the driver's own type `T` — how a driver's
    /// query API recovers what it runs a statement on, through every wrapper
    /// installed around it. `None` for another driver's.
    pub fn downcast_ref<T: Any>(self) -> Option<&'a T> {
        let held: &dyn Any = match self {
            Self::Pool(pool) => pool,
            Self::Transaction(transaction) => transaction,
        };
        held.downcast_ref()
    }
}

/// The pool, as an [`Executor`] holds it ([`Holds::Pool`]): nothing here
/// waits for a commit, so [`after_commit`] runs its work at once, a pushed job
/// is filed at once, and a policy's attempt runs on the pool itself.
pub trait HeldPool: Any + Send + Sync {
    /// Open a transaction of its own as the block [`atomic`](fn@crate::atomic)
    /// runs on the pool, committed or rolled back as the block settles.
    /// Opening sends nothing: the block's first statement does.
    fn transaction(&self) -> Arc<dyn AtomicBlock>;

    /// The records the driver's outbox keeps, for a cancel on the pool to
    /// remove one a committed unit wrote and no relay has filed — or `None`
    /// when it keeps none: a driver binding no
    /// [`OutboxStore`](crate::OutboxStore), or one whose store no relay armed.
    fn outbox(&self) -> Option<&dyn OutboxKept>;
}

/// A transaction, as an [`Executor`] holds it ([`Holds::Transaction`]): a
/// unit's, the one an atomic block opens of its own on the pool, or the
/// savepoint a block opens inside one.
pub trait HeldTransaction: Any + Send + Sync {
    /// Hold `work` until the transaction this handle's boundary settles has
    /// committed, and drop it unrun when that transaction does not commit: run
    /// after a commit (or a boundary that opened none), dropped on every other
    /// outcome. A savepoint hands what it held to the unit around it when it
    /// is kept.
    ///
    /// Held work runs where the boundary settles, so it runs under what the
    /// unit carried where it was registered: the handle captures that here,
    /// for a [`Commit`](Continuation::Commit)
    /// ([`RequestContinuation::capture`](nest_rs_core::RequestContinuation::capture)),
    /// and installs inside it its own pool, outside the transaction, under the
    /// [`current_executor_scope`] read here.
    fn after_commit(&self, work: Deferred);

    /// Open a savepoint of this transaction as an atomic block: what
    /// [`atomic`](fn@crate::atomic) runs here, and what a resilience policy
    /// runs each attempt of a call it may send again in. What a kept savepoint
    /// wrote stays its unit's to commit or roll back. Opening sends nothing:
    /// the block's first statement does.
    fn savepoint(&self) -> Arc<dyn AtomicBlock>;

    /// The outbox a job pushed here is written into, on this transaction, so
    /// it is filed after the commit and never on a rollback — or `None` when
    /// the handle keeps none: a driver binding no
    /// [`OutboxStore`](crate::OutboxStore), or one whose store no relay armed.
    /// The queue port then files the job at once.
    fn outbox(&self) -> Option<&dyn Outbox>;
}

/// Run `work` once the ambient unit of work has committed — or at once, when
/// no transaction is open that it could wait for. When the unit of work rolls
/// back, `work` is dropped and never runs.
///
/// Run at once, `work` is awaited here, inside the caller's task-locals. The
/// returned future resolves once `work` has either run or been handed to the
/// boundary to hold.
pub async fn after_commit<F>(work: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    if let Some(work) = held_or_back(work) {
        work.await;
    }
}

/// Hand `work` to the transaction the ambient executor holds, or back to run
/// at once where it holds none.
fn held_or_back<F>(work: F) -> Option<F>
where
    F: Future<Output = ()> + Send + 'static,
{
    let Some(executor) = current_executor() else {
        return Some(work);
    };
    match executor.holds() {
        Holds::Pool(_) => Some(work),
        Holds::Transaction(transaction) => {
            transaction.after_commit(Box::pin(work));
            None
        }
    }
}

/// The block [`atomic`](fn@crate::atomic) runs on the ambient executor: a
/// savepoint of the transaction it holds, a transaction of its own on the
/// pool, `None` outside any unit.
pub(crate) fn atomic_block() -> Option<Arc<dyn AtomicBlock>> {
    Some(match current_executor()?.holds() {
        Holds::Pool(pool) => pool.transaction(),
        Holds::Transaction(transaction) => transaction.savepoint(),
    })
}

/// Whether the ambient executor belongs to a request or a worker job, which
/// decides how a `Repo` treats a missing ambient ability.
///
/// It can grow within 7.x, so a `match` outside this crate keeps a `_` arm —
/// and fails closed there, as a request does:
///
/// ```compile_fail,E0004
/// use nest_rs_database::ExecutorScope;
///
/// fn unscoped(scope: ExecutorScope) -> bool {
///     match scope {
///         ExecutorScope::Request => false,
///         ExecutorScope::Job => true,
///     }
/// }
/// ```
///
/// The unit an edge opens through [`run_unit`](crate::run_unit) tags it, never
/// wider than the unit it opens in — inside a request, a job unit runs under
/// the request's scope, and so does a job's capture re-entered there — and no
/// other public function does: feature code cannot take a request's reads out
/// of its scope.
///
/// ```compile_fail,E0425
/// # use std::sync::Arc;
/// # fn handle() -> Arc<dyn nest_rs_database::Executor> { unimplemented!() }
/// # async fn reads() {}
/// let unscoped = nest_rs_database::with_job_executor(handle(), reads());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExecutorScope {
    /// A user request — a missing ambient ability is a bug, so `Repo` fails closed.
    Request,
    /// System work (cron/queue) — no principal is expected, so reads are unscoped.
    Job,
}

impl ExecutorScope {
    /// The word a line names this scope by.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Job => "job",
        }
    }
}

/// The executor a unit runs on, the scope it was installed under and the
/// binding that opened it, as one value of the ambient context.
struct AmbientExecutor {
    executor: Handle,
    scope: Option<ExecutorScope>,
    bound: Option<Bound>,
}

/// The executor itself, or the pool a continuation opens on its first touch.
enum Handle {
    Installed(Arc<dyn Executor>),
    /// The savepoint an [`InstalledSavepoint`] opened, kept as the block it
    /// settles.
    Block(Arc<dyn AtomicBlock>),
    /// A continuation of a bound unit: a [`Pool`](crate::TransactionPolicy::Pool)
    /// unit of the same binding, opened when something reads it, so carrying
    /// one costs nothing until it is used.
    Pool(OnceLock<Option<Arc<dyn Executor>>>),
}

impl AmbientExecutor {
    fn executor(&self) -> Option<Arc<dyn Executor>> {
        match &self.executor {
            Handle::Installed(executor) => Some(Arc::clone(executor)),
            Handle::Block(block) => Some(Arc::clone(block) as Arc<dyn Executor>),
            Handle::Pool(pool) => pool
                .get_or_init(|| {
                    let bound = self.bound.as_ref()?;
                    bound.pool(self.scope.unwrap_or(ExecutorScope::Request))
                })
                .clone(),
        }
    }
}

impl AmbientValue for AmbientExecutor {
    /// A continuation that can outlive the transaction runs on a pool unit of
    /// the binding that opened it — a body is polled after the commit, a batch
    /// serves every field, held work runs once the transaction is gone; one that
    /// opens its own unit (a task, a connection's next message, a job) gets
    /// none, so a detached write cannot run outside a unit of work.
    fn carry(self: Arc<Self>, into: Continuation) -> Option<Arc<Self>> {
        match into {
            Continuation::Body | Continuation::Batch | Continuation::Commit => {
                Some(Arc::new(Self {
                    executor: Handle::Pool(OnceLock::new()),
                    scope: self.scope,
                    bound: self.bound.clone(),
                }))
            }
            _ => None,
        }
    }

    /// Re-entered inside another unit, a capture runs under no wider scope
    /// than that unit's: a job's capture inside a request runs as the request.
    fn reenter(self: Arc<Self>) -> Option<Arc<Self>> {
        let Some(scope) = self.scope else {
            return Some(self);
        };
        let running = nest_rs_core::ambient::<AmbientExecutor>();
        let installed = within(running.as_deref(), scope);
        if installed == scope {
            return Some(self);
        }
        Some(Arc::new(Self {
            executor: match &self.executor {
                Handle::Installed(executor) => Handle::Installed(Arc::clone(executor)),
                Handle::Block(block) => Handle::Block(Arc::clone(block)),
                // Its pool opens under the scope it now runs under.
                Handle::Pool(_) => Handle::Pool(OnceLock::new()),
            },
            scope: Some(installed),
            bound: self.bound.clone(),
        }))
    }
}

/// `scope`, or [`Request`](ExecutorScope::Request) inside a unit `running`
/// tags anything but [`Job`](ExecutorScope::Job): an install never widens the
/// unit it runs in, and outside every unit it tags what it is given.
fn within(running: Option<&AmbientExecutor>, scope: ExecutorScope) -> ExecutorScope {
    match running.map(|ambient| ambient.scope) {
        None | Some(Some(ExecutorScope::Job)) => scope,
        Some(_) => ExecutorScope::Request,
    }
}

/// The scope an install of `scope` is tagged with where this runs: never wider
/// than the running unit's.
pub fn installed_scope(scope: ExecutorScope) -> ExecutorScope {
    within(nest_rs_core::ambient::<AmbientExecutor>().as_deref(), scope)
}

/// The installed ambient executor, or `None` outside any scope. An
/// ORM-specific `Repo` calls this and recovers its own handle off what the
/// executor holds ([`Holds::downcast_ref`]). A miss (mismatched `Module` +
/// `Repo`) is logged at `error` and treated as no ambient executor, so the
/// operation fails rather than panicking or reading "no rows".
pub fn current_executor() -> Option<Arc<dyn Executor>> {
    nest_rs_core::ambient::<AmbientExecutor>().and_then(|ambient| ambient.executor())
}

/// The installed ambient executor scope, or `None` outside any scope.
pub fn current_executor_scope() -> Option<ExecutorScope> {
    nest_rs_core::ambient::<AmbientExecutor>().and_then(|ambient| ambient.scope)
}

/// The binding that opened the running unit, carried with its executor.
pub(crate) fn current_bound() -> Option<Bound> {
    nest_rs_core::ambient::<AmbientExecutor>().and_then(|ambient| ambient.bound.clone())
}

/// Install `executor` without tagging a scope: it keeps the scope installed
/// where `fut` runs, and an untagged one is **fail-closed** like a request —
/// only [`with_job_executor`] grants unscoped reads.
pub fn with_executor<F: Future>(
    executor: Arc<dyn Executor>,
    fut: F,
) -> impl Future<Output = F::Output> {
    nest_rs_core::__private::with_ambient_from(
        move || {
            let running = nest_rs_core::ambient::<AmbientExecutor>();
            Arc::new(AmbientExecutor {
                executor: Handle::Installed(executor),
                scope: running.as_ref().and_then(|ambient| ambient.scope),
                bound: running.and_then(|ambient| ambient.bound.clone()),
            })
        },
        fut,
    )
}

/// A savepoint of the transaction the ambient executor holds, installed as the
/// ambient executor of what its holder polls under it: what [`with_executor`]
/// derives around a future, derived once, where the savepoint is opened, and
/// entered around each poll — for a caller polling in place a future it cannot
/// move under an install.
pub struct InstalledSavepoint(ExtendedContext);

impl InstalledSavepoint {
    /// A savepoint of the ambient executor's transaction
    /// ([`HeldTransaction::savepoint`]), under the scope and the binding
    /// installed here, or `None` where it holds none: outside any unit, and on
    /// the pool. Nothing is sent to the database until a statement runs under
    /// it.
    pub fn open() -> Option<Self> {
        let running = nest_rs_core::ambient::<AmbientExecutor>()?;
        let block = match running.executor()?.holds() {
            Holds::Transaction(transaction) => transaction.savepoint(),
            Holds::Pool(_) => return None,
        };
        Some(Self(ExtendedContext::with(Arc::new(AmbientExecutor {
            executor: Handle::Block(block),
            scope: running.scope,
            bound: running.bound.clone(),
        }))))
    }

    /// Run `poll` with the savepoint as the ambient executor: synchronous, for
    /// one poll of what runs inside it.
    pub fn enter<R>(&self, poll: impl FnOnce() -> R) -> R {
        self.0.enter(poll)
    }

    /// The savepoint settling, as [`AtomicBlock::settle`] says. What ran under
    /// it is gone by then: the context goes first, so a handle on the block
    /// still held past here is one that escaped.
    pub fn settle(
        self,
        keep: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::AtomicError>> + Send>> {
        let block =
            self.0
                .get::<AmbientExecutor>()
                .and_then(|installed| match &installed.executor {
                    Handle::Block(block) => Some(Arc::clone(block)),
                    Handle::Installed(_) | Handle::Pool(_) => None,
                });
        drop(self);
        let Some(block) = block else {
            unreachable!("an installed savepoint's context carries the block it was opened with");
        };
        block.settle(keep)
    }
}

/// Install `executor` and tag the scope as a request — the path on which a
/// `Repo` fails closed when no ambient authorization context is present.
///
/// Framework plumbing, beside [`with_job_executor`]: a unit is the edge's own,
/// opened through [`run_unit`](crate::run_unit), and a service test's the
/// testing harness's; a framework suite installs a handle of its own here.
pub fn with_request_executor<F: Future>(
    executor: Arc<dyn Executor>,
    fut: F,
) -> impl Future<Output = F::Output> {
    install(executor, ExecutorScope::Request, fut)
}

/// Install `executor` and tag the scope as a worker job — the path on
/// which a `Repo` runs unscoped (no principal ⇒ system work).
///
/// It never widens the unit it runs in: inside a request, a continuation of
/// one or an untagged install, `fut` runs under the request scope, so a unit
/// that fails closed stays closed. Outside every unit it grants the job scope
/// to whatever handle it is given, which is why it is no public function: a
/// framework suite running system work on a handle of its own calls it, and a
/// service's system work is a queue attempt or a tick of its own.
pub fn with_job_executor<F: Future>(
    executor: Arc<dyn Executor>,
    fut: F,
) -> impl Future<Output = F::Output> {
    install(executor, ExecutorScope::Job, fut)
}

/// Install `executor` under `scope`, keeping the binding of the unit it runs
/// in, or under [`Request`](ExecutorScope::Request) inside a unit not tagged
/// [`Job`](ExecutorScope::Job).
fn install<F: Future>(
    executor: Arc<dyn Executor>,
    scope: ExecutorScope,
    fut: F,
) -> impl Future<Output = F::Output> {
    nest_rs_core::__private::with_ambient_from(
        move || {
            let running = nest_rs_core::ambient::<AmbientExecutor>();
            Arc::new(AmbientExecutor {
                executor: Handle::Installed(executor),
                scope: Some(within(running.as_deref(), scope)),
                bound: running.and_then(|ambient| ambient.bound.clone()),
            })
        },
        fut,
    )
}

/// Install a unit's `executor` under `scope`, with the binding that opened it,
/// never wider than the unit it opens in, as [`install`].
pub(crate) fn install_unit<F: Future>(
    executor: Arc<dyn Executor>,
    scope: ExecutorScope,
    bound: Bound,
    fut: F,
) -> impl Future<Output = F::Output> {
    nest_rs_core::__private::with_ambient_from(
        move || {
            let running = nest_rs_core::ambient::<AmbientExecutor>();
            Arc::new(AmbientExecutor {
                executor: Handle::Installed(executor),
                scope: Some(within(running.as_deref(), scope)),
                bound: Some(bound),
            })
        },
        fut,
    )
}

#[cfg(test)]
mod tests {
    use nest_rs_core::RequestContinuation;

    use super::*;

    /// A pool no test opens a block on, keeping no outbox.
    struct StubExecutor;

    impl Executor for StubExecutor {
        fn holds(&self) -> Holds<'_> {
            Holds::Pool(self)
        }
    }

    impl HeldPool for StubExecutor {
        fn transaction(&self) -> Arc<dyn AtomicBlock> {
            unreachable!("no test opens a block on the stub")
        }

        fn outbox(&self) -> Option<&dyn OutboxKept> {
            None
        }
    }

    fn stub() -> Arc<dyn Executor> {
        Arc::new(StubExecutor)
    }

    /// Whether the ambient executor holds a `T`.
    fn holds<T: Any>() -> bool {
        current_executor().is_some_and(|e| e.holds().downcast_ref::<T>().is_some())
    }

    /// A unit holding a transaction: its savepoint says how it was settled,
    /// and how many handles were on it then.
    struct InTransaction(Arc<std::sync::Mutex<Vec<(bool, usize)>>>);

    impl Executor for InTransaction {
        fn holds(&self) -> Holds<'_> {
            Holds::Transaction(self)
        }
    }

    impl HeldTransaction for InTransaction {
        fn after_commit(&self, _work: Deferred) {
            unreachable!("no test holds work on this unit")
        }

        fn savepoint(&self) -> Arc<dyn AtomicBlock> {
            Arc::new(InSavepoint(Arc::clone(&self.0)))
        }

        fn outbox(&self) -> Option<&dyn Outbox> {
            None
        }
    }

    struct InSavepoint(Arc<std::sync::Mutex<Vec<(bool, usize)>>>);

    impl Executor for InSavepoint {
        fn holds(&self) -> Holds<'_> {
            Holds::Transaction(self)
        }
    }

    impl HeldTransaction for InSavepoint {
        fn after_commit(&self, _work: Deferred) {
            unreachable!("no test holds work on this savepoint")
        }

        fn savepoint(&self) -> Arc<dyn AtomicBlock> {
            unreachable!("no test nests a savepoint in this one")
        }

        fn outbox(&self) -> Option<&dyn Outbox> {
            None
        }
    }

    impl AtomicBlock for InSavepoint {
        fn settle(
            self: Arc<Self>,
            ok: bool,
        ) -> Pin<Box<dyn Future<Output = Result<(), crate::AtomicError>> + Send>> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((ok, Arc::strong_count(&self)));
            Box::pin(std::future::ready(Ok(())))
        }
    }

    #[tokio::test]
    async fn an_installed_savepoint_is_entered_under_its_units_scope_and_settles_with_no_handle_left()
     {
        let settled = Arc::new(std::sync::Mutex::new(Vec::new()));
        let unit: Arc<dyn Executor> = Arc::new(InTransaction(Arc::clone(&settled)));

        assert!(InstalledSavepoint::open().is_none(), "outside every unit");
        let on_pool = with_job_executor(stub(), async { InstalledSavepoint::open().is_none() });
        assert!(on_pool.await, "a handle holding no transaction");

        let savepoint = with_job_executor(unit, async { InstalledSavepoint::open() })
            .await
            .expect("the unit holds a transaction");
        assert!(
            current_executor().is_none(),
            "nothing is installed between two polls"
        );
        for _poll in 0..2 {
            savepoint.enter(|| {
                assert!(holds::<InSavepoint>());
                assert_eq!(current_executor_scope(), Some(ExecutorScope::Job));
            });
        }

        assert!(savepoint.settle(true).await.is_ok());
        assert_eq!(
            *settled
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [(true, 1)],
            "the context holding the block is gone before it settles"
        );
    }

    #[tokio::test]
    async fn no_ambient_state_outside_any_scope() {
        assert!(current_executor().is_none());
        assert!(current_executor_scope().is_none());
    }

    #[tokio::test]
    async fn with_executor_installs_but_does_not_tag() {
        with_executor(stub(), async {
            assert!(current_executor().is_some());
            assert!(current_executor_scope().is_none());
        })
        .await;
    }

    #[tokio::test]
    async fn with_request_executor_tags_request() {
        with_request_executor(stub(), async {
            assert_eq!(current_executor_scope(), Some(ExecutorScope::Request));
            assert!(current_executor().is_some());
        })
        .await;
    }

    #[tokio::test]
    async fn with_job_executor_tags_job() {
        with_job_executor(stub(), async {
            assert_eq!(current_executor_scope(), Some(ExecutorScope::Job));
            assert!(current_executor().is_some());
        })
        .await;
    }

    #[tokio::test]
    async fn scope_unwinds_on_exit() {
        with_request_executor(stub(), async {}).await;
        assert!(current_executor().is_none());
        assert!(current_executor_scope().is_none());
    }

    #[tokio::test]
    async fn a_nested_install_shadows_then_restores_the_outer_scope() {
        with_job_executor(stub(), async {
            with_request_executor(Arc::new(OnPool), async {
                assert_eq!(current_executor_scope(), Some(ExecutorScope::Request));
                assert!(holds::<OnPool>());
            })
            .await;
            assert_eq!(current_executor_scope(), Some(ExecutorScope::Job));
        })
        .await;
    }

    #[tokio::test]
    async fn a_job_install_never_widens_a_unit_that_fails_closed() {
        let job = || {
            with_job_executor(Arc::new(OnPool), async {
                assert!(holds::<OnPool>());
                current_executor_scope()
            })
        };
        let streamed = with_request_executor(stub(), async {
            RequestContinuation::capture(Continuation::Body).expect("a request carries a body")
        })
        .await;

        let ran = [
            ("a request", with_request_executor(stub(), job()).await),
            ("an untagged install", with_executor(stub(), job()).await),
            ("a request's streaming body", streamed.scope(job()).await),
            ("a job", with_job_executor(stub(), job()).await),
            ("no unit", job().await),
        ];

        assert_eq!(
            ran,
            [
                ("a request", Some(ExecutorScope::Request)),
                ("an untagged install", Some(ExecutorScope::Request)),
                ("a request's streaming body", Some(ExecutorScope::Request)),
                ("a job", Some(ExecutorScope::Job)),
                ("no unit", Some(ExecutorScope::Job)),
            ],
        );
    }

    #[tokio::test]
    async fn a_jobs_capture_re_entered_inside_a_request_runs_under_the_request() {
        let (captured, task) = with_job_executor(stub(), async {
            (
                RequestContinuation::capture(Continuation::Commit)
                    .expect("a job carries its commit"),
                nest_rs_core::TaskContext::capture(Continuation::Commit),
            )
        })
        .await;
        let scoped = || captured.scope(async { current_executor_scope() });

        let ran = [
            ("a request", with_request_executor(stub(), scoped()).await),
            (
                "a request, entered",
                with_request_executor(stub(), async { captured.enter(current_executor_scope) })
                    .await,
            ),
            (
                "a task carried into a request",
                with_request_executor(stub(), task.carry(async { current_executor_scope() })).await,
            ),
            ("an untagged install", with_executor(stub(), scoped()).await),
            ("a job", with_job_executor(stub(), scoped()).await),
            ("no unit", scoped().await),
        ];

        assert_eq!(
            ran,
            [
                ("a request", Some(ExecutorScope::Request)),
                ("a request, entered", Some(ExecutorScope::Request)),
                (
                    "a task carried into a request",
                    Some(ExecutorScope::Request)
                ),
                ("an untagged install", Some(ExecutorScope::Request)),
                ("a job", Some(ExecutorScope::Job)),
                ("no unit", Some(ExecutorScope::Job)),
            ],
        );
    }

    #[tokio::test]
    async fn what_a_handle_holds_downcasts_to_the_drivers_own_type_and_to_no_other() {
        with_request_executor(stub(), async {
            let e = current_executor().expect("installed");
            assert!(e.holds().downcast_ref::<StubExecutor>().is_some());
            assert!(e.holds().downcast_ref::<OnPool>().is_none());
        })
        .await;
    }

    /// Another handle than the stub, to tell an inner install from an outer.
    struct OnPool;

    impl Executor for OnPool {
        fn holds(&self) -> Holds<'_> {
            Holds::Pool(self)
        }
    }

    impl HeldPool for OnPool {
        fn transaction(&self) -> Arc<dyn AtomicBlock> {
            unreachable!("no test opens a block on this pool")
        }

        fn outbox(&self) -> Option<&dyn OutboxKept> {
            None
        }
    }

    fn crossing(executor: Arc<dyn Executor>, into: Continuation) -> Option<Arc<AmbientExecutor>> {
        Arc::new(AmbientExecutor {
            executor: Handle::Installed(executor),
            scope: Some(ExecutorScope::Request),
            bound: None,
        })
        .carry(into)
    }

    #[test]
    fn a_continuation_that_opens_its_own_unit_carries_no_executor() {
        for into in [
            Continuation::Task,
            Continuation::Connection,
            Continuation::Envelope,
        ] {
            assert!(crossing(stub(), into).is_none(), "{into:?}");
        }
    }

    #[test]
    fn a_continuation_that_can_outlive_the_transaction_never_holds_it() {
        for into in [
            Continuation::Body,
            Continuation::Batch,
            Continuation::Commit,
        ] {
            let carried = crossing(stub(), into).expect("the scope crosses");
            assert!(
                carried.executor().is_none(),
                "{into:?} runs on a pool its binding opens, and none is bound here",
            );
            assert_eq!(
                carried.scope,
                Some(ExecutorScope::Request),
                "{into:?} keeps the tag"
            );
        }
    }

    #[tokio::test]
    async fn an_untagged_install_keeps_the_scope_already_installed() {
        with_request_executor(stub(), async {
            with_executor(Arc::new(OnPool), async {
                assert_eq!(current_executor_scope(), Some(ExecutorScope::Request));
                assert!(holds::<OnPool>());
            })
            .await;
        })
        .await;
    }

    #[tokio::test]
    async fn an_untagged_install_built_as_an_argument_keeps_the_scope_it_runs_under() {
        let untagged = with_executor(Arc::new(OnPool), async {
            assert_eq!(current_executor_scope(), Some(ExecutorScope::Job));
            assert!(holds::<OnPool>());
        });
        with_job_executor(stub(), untagged).await;
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
                std::mem::size_of_val(&with_executor(stub(), heavy())),
            ),
            (
                "with_request_executor",
                std::mem::size_of_val(&with_request_executor(stub(), heavy())),
            ),
            (
                "with_job_executor",
                std::mem::size_of_val(&with_job_executor(stub(), heavy())),
            ),
        ];
        for (installer, size) in installed {
            assert!(
                size < alone + 1024,
                "{installer} weighs {size} bytes around a {alone}-byte unit",
            );
        }
    }

    /// A boundary's transaction, reduced to a place to keep held work.
    #[derive(Default)]
    struct Holding(std::sync::Mutex<Vec<Deferred>>);

    impl Executor for Holding {
        fn holds(&self) -> Holds<'_> {
            Holds::Transaction(self)
        }
    }

    impl HeldTransaction for Holding {
        fn after_commit(&self, work: Deferred) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(work);
        }

        fn savepoint(&self) -> Arc<dyn AtomicBlock> {
            unreachable!("no test opens a savepoint on this boundary")
        }

        fn outbox(&self) -> Option<&dyn Outbox> {
            None
        }
    }

    fn ran() -> (
        Arc<std::sync::atomic::AtomicBool>,
        impl Future<Output = ()> + Send,
    ) {
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let set = flag.clone();
        (flag, async move {
            set.store(true, std::sync::atomic::Ordering::SeqCst);
        })
    }

    #[tokio::test]
    async fn after_commit_runs_at_once_outside_any_scope() {
        let (flag, work) = ran();
        after_commit(work).await;
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_pool_has_nothing_to_wait_for_and_runs_the_work_inside_the_callers_scope() {
        let seen = Arc::new(std::sync::Mutex::new(None));
        let record = seen.clone();
        with_request_executor(stub(), async move {
            after_commit(async move {
                *record
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(current_executor_scope());
            })
            .await;
        })
        .await;
        assert_eq!(
            *seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            Some(Some(ExecutorScope::Request)),
        );
    }

    #[tokio::test]
    async fn a_transaction_keeps_the_work_until_its_boundary_runs_it() {
        let holding = Arc::new(Holding::default());
        let (flag, work) = ran();
        with_request_executor(holding.clone(), after_commit(work)).await;
        assert!(
            !flag.load(std::sync::atomic::Ordering::SeqCst),
            "held work does not run before its boundary settles",
        );

        let held: Vec<Deferred> = std::mem::take(
            &mut *holding
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        assert_eq!(held.len(), 1);
        for work in held {
            work.await;
        }
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
    }
}
