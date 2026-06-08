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
/// (mismatched `Module` + `Repo`) and should `panic!` with a clear message
/// during boot tests, never surface as a runtime "no rows".
pub trait Executor: Any + Send + Sync + 'static {
    /// Downcast handle. Used by an ORM-specific `Repo` to recover its
    /// concrete executor type from the ambient `Arc<dyn Executor>`.
    fn as_any(&self) -> &dyn Any;
}

/// Whether the ambient executor belongs to a request or a worker job. An
/// ORM's `Repo` reads this back to fail closed when a request path lacks
/// an ambient authorization context (a missing principal on a worker is
/// expected — it's system work; on a request it's a bug).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutorScope {
    Request,
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
/// two paths.
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
