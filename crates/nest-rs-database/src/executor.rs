use std::any::Any;
use std::future::Future;
use std::sync::Arc;

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
}
