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
/// The trait is **object-safe** so the engine can carry it as
/// `Arc<dyn Executor>` without naming a concrete ORM. The concrete handle
/// (a SeaORM `Executor` enum, a `sqlx::Pool`, a `diesel_async::Connection`,
/// …) implements this trait; an ORM-specific `Repo` recovers the concrete
/// type via [`Executor::as_any`] when it needs to issue a query.
///
/// Downcasting is the documented seam: this crate stays free of every
/// candidate ORM's query API, and each `Repo` knows exactly which executor
/// shape its `Module` installs. A downcast miss is a framework bug
/// (mismatched `Module` + `Repo`); the contract is **log at `error` and
/// degrade to `None`** — the `Repo` then fails the operation (no ambient
/// executor), so the request errors loudly instead of panicking a worker
/// thread or silently reading "no rows".
pub trait Executor: Any + Send + Sync + 'static {
    /// Downcast handle. Used by an ORM-specific `Repo` to recover its
    /// concrete executor type from the ambient `Arc<dyn Executor>`.
    fn as_any(&self) -> &dyn Any;

    /// A handle on the same database **outside** this executor's transaction,
    /// or `None` when there is nothing to step out of (already a pool) or the
    /// ORM cannot produce one.
    ///
    /// A transport that has *proven* an operation cannot write installs this
    /// for the operation's duration, so the work runs without opening — and
    /// without pinning a connection to — the request transaction. The one
    /// caller today is the GraphQL endpoint: every operation arrives as a
    /// POST, so the HTTP boundary hands even a pure query a transaction it
    /// will never need.
    ///
    /// **Only ever pass work that cannot write.** A mutation on the returned
    /// handle loses atomicity and rollback. The default `None` is therefore
    /// the fail-closed answer: an ORM that ignores this keeps the request
    /// executor it was given.
    fn non_transactional(&self) -> Option<Arc<dyn Executor>> {
        None
    }

    /// Hold `work` until the transaction this executor's boundary settles has
    /// committed, and drop it unrun when that transaction does not commit —
    /// or hand it back, `Some(work)`, when this executor settles no transaction
    /// the work could wait for, and the caller runs it at once.
    ///
    /// An executor that holds the work owes all of it: the boundary that settles
    /// the transaction runs it after a commit (or after a boundary that
    /// succeeded without opening one, since then nothing could roll back), and
    /// drops it on a rollback, a failed commit, a poisoned transaction or an
    /// escaped handle — whatever made the boundary write nothing.
    ///
    /// The default hands the work back, and that is right for a pool, which has
    /// nothing to wait for, and for a transaction a **caller** opened and will
    /// commit itself, which the framework cannot see: holding work for a commit
    /// nothing will report would mean never running it. A driver whose
    /// boundaries settle a transaction overrides this, or an event emitted
    /// inside one is dispatched before the transaction it reports has landed.
    fn after_commit(&self, work: Deferred) -> Option<Deferred> {
        Some(work)
    }
}

/// Run `work` once the ambient unit of work has committed — or at once, when
/// no transaction is open that it could wait for. When the unit of work rolls
/// back, `work` is dropped and never runs.
///
/// This is the seam an effect that must not be seen before a commit goes
/// through: an event announces a fact, and a fact the transaction then rolls
/// back never happened, so `nest_rs_events::EventBus::emit` dispatches through
/// here. What waits is decided by the ambient [`Executor`]'s
/// [`after_commit`](Executor::after_commit); outside any scope, or on an
/// executor with nothing to wait for, `work` is awaited right here, inside the
/// caller's task-locals.
///
/// The returned future resolves once `work` has either run or been handed to
/// the boundary to hold, so a caller cannot tell the two apart, and is not
/// meant to.
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

/// Whether the ambient executor belongs to a request or a worker job. An
/// ORM's `Repo` reads this back to fail closed when a request path lacks
/// an ambient authorization context (a missing principal on a worker is
/// expected — it's system work; on a request it's a bug).
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

/// Install `executor` without tagging a scope. Prefer the request/job
/// variants at framework boundaries so authorization can distinguish the
/// two paths. An untagged (unset) scope is treated as **fail-closed** by a
/// scope-aware `Repo`: with no ambient ability it denies every row, exactly
/// like a request — only [`with_job_executor`] grants unscoped reads.
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

    /// A boundary's transaction, reduced to what `after_commit` asks of it: a
    /// place to keep the work until the boundary settles.
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

    /// The default is the pool's answer — nothing to wait for — and the work
    /// runs right there, inside the caller's task-locals: a handler that emits
    /// on a safe route keeps the executor and scope it emitted from.
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

    /// The executor that holds the work owns it from then on: `after_commit`
    /// returns without running it, and it runs when — and only if — the
    /// boundary hands it on.
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
