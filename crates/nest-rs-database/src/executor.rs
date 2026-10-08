use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Work held until the ambient unit of work commits — what [`after_commit`]
/// hands an [`Executor`] to keep.
pub type Deferred = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// An ambient handle to a unit of database work, installed in the
/// task-local for the lifetime of a request or a worker job.
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
    fn after_commit(&self, work: Deferred) -> Option<Deferred> {
        Some(work)
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

tokio::task_local! {
    static EXECUTOR: Arc<dyn Executor>;
    static EXECUTOR_SCOPE: ExecutorScope;
}

/// The installed ambient executor, or `None` outside any scope. An
/// ORM-specific `Repo` calls this and downcasts via [`Executor::as_any`].
pub fn current_executor() -> Option<Arc<dyn Executor>> {
    EXECUTOR.try_with(Arc::clone).ok()
}

/// The installed ambient executor scope, or `None` outside any scope.
pub fn current_executor_scope() -> Option<ExecutorScope> {
    EXECUTOR_SCOPE.try_with(Clone::clone).ok()
}

/// Install `executor` without tagging a scope. An untagged scope is
/// **fail-closed** like a request: only [`with_job_executor`] grants unscoped reads.
pub async fn with_executor<F: Future>(executor: Arc<dyn Executor>, fut: F) -> F::Output {
    EXECUTOR.scope(executor, fut).await
}

/// Install `executor` and tag the scope as a request — the path on which a
/// `Repo` fails closed when no ambient authorization context is present.
pub async fn with_request_executor<F: Future>(executor: Arc<dyn Executor>, fut: F) -> F::Output {
    EXECUTOR
        .scope(executor, EXECUTOR_SCOPE.scope(ExecutorScope::Request, fut))
        .await
}

/// Install `executor` and tag the scope as a worker job — the path on
/// which a `Repo` runs unscoped (no principal ⇒ system work).
pub async fn with_job_executor<F: Future>(executor: Arc<dyn Executor>, fut: F) -> F::Output {
    EXECUTOR
        .scope(executor, EXECUTOR_SCOPE.scope(ExecutorScope::Job, fut))
        .await
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
