use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_core::{AmbientValue, Continuation};

/// Work held until the ambient unit of work commits — what [`after_commit`]
/// hands an [`Executor`] to keep.
pub type Deferred = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// An ambient handle to a unit of database work, installed in the ambient
/// context for the lifetime of a request or a worker job.
///
/// An ORM-specific `Repo` recovers its concrete handle via
/// [`Executor::as_any`]. A downcast miss (mismatched `Module` + `Repo`) is
/// logged at `error` and treated as no ambient executor, so the operation fails
/// rather than panicking or reading "no rows".
pub trait Executor: Any + Send + Sync + 'static {
    /// Downcast handle, for an ORM-specific `Repo` to recover its concrete type.
    fn as_any(&self) -> &dyn Any;

    /// A handle on the same database **outside** this executor's transaction,
    /// or `None` when there is nothing to step out of (already a pool) or the
    /// ORM cannot produce one.
    ///
    /// **Only ever pass work that cannot write**: a mutation on the returned
    /// handle loses atomicity and rollback.
    fn non_transactional(&self) -> Option<Arc<dyn Executor>> {
        None
    }

    /// Hold `work` until the transaction this executor's boundary settles has
    /// committed, and drop it unrun when that transaction does not commit —
    /// or hand it back, `Some(work)`, when this executor settles no transaction
    /// the work could wait for, and the caller runs it at once.
    ///
    /// An executor that holds the work owes all of it: run after a commit (or a
    /// boundary that opened none), dropped on every other outcome. The default
    /// hands it back — right for a pool and for a transaction a caller opened
    /// itself; a driver whose boundaries settle a transaction overrides it.
    ///
    /// Held work runs where the boundary settles, so it runs under what the
    /// unit carried where it was registered: the executor captures that here,
    /// for a [`Commit`](Continuation::Commit)
    /// ([`RequestContinuation::capture`](nest_rs_core::RequestContinuation::capture)),
    /// and installs inside it its handle outside the transaction, under the
    /// [`current_executor_scope`] read here — the capture carries no other
    /// driver's executor, nor its scope.
    fn after_commit(&self, work: Deferred) -> Option<Deferred> {
        Some(work)
    }

    /// A handle on the pool this executor draws from, outside any transaction:
    /// what a continuation that can outlive the transaction runs on (a
    /// streaming body, a DataLoader batch, after-commit work). `None` when there
    /// is none to give, and the continuation then runs without an executor.
    #[doc(hidden)]
    fn __pool(&self) -> Option<Arc<dyn Executor>> {
        None
    }
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
    let work: Deferred = Box::pin(work);
    let work = match current_executor() {
        Some(executor) => match executor.after_commit(work) {
            Some(work) => work,
            None => return,
        },
        None => work,
    };
    work.await;
}

/// Whether the ambient executor belongs to a request or a worker job, which
/// decides how a `Repo` treats a missing ambient ability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutorScope {
    /// A user request — a missing ambient ability is a bug, so `Repo` fails closed.
    Request,
    /// System work (cron/queue) — no principal is expected, so reads are unscoped.
    Job,
}

/// The executor a unit runs on and the scope it was installed under, as one
/// value of the ambient context.
struct AmbientExecutor {
    executor: Arc<dyn Executor>,
    scope: Option<ExecutorScope>,
}

impl AmbientValue for AmbientExecutor {
    /// A continuation that can outlive the transaction runs on the pool — a
    /// body is polled after the commit, a batch serves every field, held work
    /// runs once the transaction is gone; one that opens its own unit (a task,
    /// a connection's next message, a job) gets none, so a detached write
    /// cannot run outside a unit of work.
    fn carry(self: Arc<Self>, into: Continuation) -> Option<Arc<Self>> {
        match into {
            Continuation::Body | Continuation::Batch | Continuation::Commit => {
                Some(Arc::new(Self {
                    executor: self.executor.__pool()?,
                    scope: self.scope,
                }))
            }
            _ => None,
        }
    }
}

/// The installed ambient executor, or `None` outside any scope. An
/// ORM-specific `Repo` calls this and downcasts via [`Executor::as_any`].
pub fn current_executor() -> Option<Arc<dyn Executor>> {
    nest_rs_core::ambient::<AmbientExecutor>().map(|ambient| Arc::clone(&ambient.executor))
}

/// The installed ambient executor scope, or `None` outside any scope.
pub fn current_executor_scope() -> Option<ExecutorScope> {
    nest_rs_core::ambient::<AmbientExecutor>().and_then(|ambient| ambient.scope)
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
            Arc::new(AmbientExecutor {
                executor,
                scope: current_executor_scope(),
            })
        },
        fut,
    )
}

/// Install `executor` and tag the scope as a request — the path on which a
/// `Repo` fails closed when no ambient authorization context is present.
pub fn with_request_executor<F: Future>(
    executor: Arc<dyn Executor>,
    fut: F,
) -> impl Future<Output = F::Output> {
    install(executor, ExecutorScope::Request, fut)
}

/// Install `executor` and tag the scope as a worker job — the path on
/// which a `Repo` runs unscoped (no principal ⇒ system work).
pub fn with_job_executor<F: Future>(
    executor: Arc<dyn Executor>,
    fut: F,
) -> impl Future<Output = F::Output> {
    install(executor, ExecutorScope::Job, fut)
}

fn install<F: Future>(
    executor: Arc<dyn Executor>,
    scope: ExecutorScope,
    fut: F,
) -> impl Future<Output = F::Output> {
    let ambient = AmbientExecutor {
        executor,
        scope: Some(scope),
    };
    nest_rs_core::with_ambient(Arc::new(ambient), fut)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubExecutor;
    impl Executor for StubExecutor {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn stub() -> Arc<dyn Executor> {
        Arc::new(StubExecutor)
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
    async fn nested_scope_shadows_outer() {
        with_request_executor(stub(), async {
            assert_eq!(current_executor_scope(), Some(ExecutorScope::Request));
            with_job_executor(stub(), async {
                assert_eq!(current_executor_scope(), Some(ExecutorScope::Job));
            })
            .await;
            assert_eq!(current_executor_scope(), Some(ExecutorScope::Request));
        })
        .await;
    }

    #[tokio::test]
    async fn downcast_round_trips_the_concrete_type() {
        with_request_executor(stub(), async {
            let e = current_executor().expect("installed");
            assert!(e.as_any().is::<StubExecutor>());
        })
        .await;
    }

    /// A transaction that knows the pool it was opened on.
    struct OnPool;

    impl Executor for OnPool {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn __pool(&self) -> Option<Arc<dyn Executor>> {
            Some(stub())
        }
    }

    fn crossing(executor: Arc<dyn Executor>, into: Continuation) -> Option<Arc<AmbientExecutor>> {
        Arc::new(AmbientExecutor {
            executor,
            scope: Some(ExecutorScope::Request),
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
            assert!(crossing(Arc::new(OnPool), into).is_none(), "{into:?}");
        }
    }

    #[test]
    fn a_continuation_that_can_outlive_the_transaction_steps_down_to_its_pool() {
        for into in [
            Continuation::Body,
            Continuation::Batch,
            Continuation::Commit,
        ] {
            let carried = crossing(Arc::new(OnPool), into).expect("the pool crosses");
            assert!(
                carried.executor.as_any().is::<StubExecutor>(),
                "{into:?} runs on the pool, never the transaction",
            );
            assert_eq!(
                carried.scope,
                Some(ExecutorScope::Request),
                "{into:?} keeps the tag"
            );
        }
    }

    #[test]
    fn an_executor_with_no_pool_to_give_crosses_nowhere() {
        for into in [
            Continuation::Body,
            Continuation::Batch,
            Continuation::Commit,
        ] {
            assert!(crossing(stub(), into).is_none(), "{into:?}");
        }
    }

    #[tokio::test]
    async fn an_untagged_install_keeps_the_scope_already_installed() {
        with_request_executor(stub(), async {
            with_executor(Arc::new(OnPool), async {
                assert_eq!(current_executor_scope(), Some(ExecutorScope::Request));
                assert!(current_executor().is_some_and(|e| e.as_any().is::<OnPool>()));
            })
            .await;
        })
        .await;
    }

    #[tokio::test]
    async fn an_untagged_install_built_as_an_argument_keeps_the_scope_it_runs_under() {
        let untagged = with_executor(Arc::new(OnPool), async {
            assert_eq!(current_executor_scope(), Some(ExecutorScope::Job));
            assert!(current_executor().is_some_and(|e| e.as_any().is::<OnPool>()));
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
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn after_commit(&self, work: Deferred) -> Option<Deferred> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(work);
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
    async fn an_executor_with_nothing_to_wait_for_runs_the_work_inside_the_callers_scope() {
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
    async fn an_executor_that_holds_the_work_keeps_it_until_the_boundary_runs_it() {
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
