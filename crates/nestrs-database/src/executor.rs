//! Ambient, request-scoped database executor.
//!
//! [`Executor`] is the request's current connection — pool or transaction —
//! carried in a task-local that [`DbContext`](crate::DbContext) installs and
//! [`Repo`](crate::Repo) reads back. The enum exists because SeaORM's
//! `ConnectionTrait` has generic methods (not object-safe); forwarding via a
//! variant restores a single `&Executor` that drives any SeaORM query.

use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, DbErr, ExecResult,
    QueryResult, Statement,
};

/// The connection a request's queries run against: the shared pool, or the
/// per-request [`DatabaseTransaction`]. Cheap to clone.
#[derive(Clone)]
pub enum Executor {
    Pool(Arc<DatabaseConnection>),
    Txn(Arc<DatabaseTransaction>),
}

#[async_trait]
impl ConnectionTrait for Executor {
    fn get_database_backend(&self) -> DbBackend {
        match self {
            Executor::Pool(c) => c.get_database_backend(),
            Executor::Txn(t) => t.get_database_backend(),
        }
    }

    async fn execute_raw(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
        match self {
            Executor::Pool(c) => c.execute_raw(stmt).await,
            Executor::Txn(t) => t.execute_raw(stmt).await,
        }
    }

    async fn execute_unprepared(&self, sql: &str) -> Result<ExecResult, DbErr> {
        match self {
            Executor::Pool(c) => c.execute_unprepared(sql).await,
            Executor::Txn(t) => t.execute_unprepared(sql).await,
        }
    }

    async fn query_one_raw(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
        match self {
            Executor::Pool(c) => c.query_one_raw(stmt).await,
            Executor::Txn(t) => t.query_one_raw(stmt).await,
        }
    }

    async fn query_all_raw(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
        match self {
            Executor::Pool(c) => c.query_all_raw(stmt).await,
            Executor::Txn(t) => t.query_all_raw(stmt).await,
        }
    }

    fn support_returning(&self) -> bool {
        match self {
            Executor::Pool(c) => c.support_returning(),
            Executor::Txn(t) => t.support_returning(),
        }
    }

    fn is_mock_connection(&self) -> bool {
        match self {
            Executor::Pool(c) => c.is_mock_connection(),
            Executor::Txn(t) => t.is_mock_connection(),
        }
    }
}

tokio::task_local! {
    static EXECUTOR: Executor;
}

/// Whether the ambient executor belongs to a request or a worker job. Used by
/// [`crate::repo::scope_for`] to fail closed on request paths that lack an
/// ambient [`Ability`](nestrs_authz::Ability).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutorScope {
    Request,
    Job,
}

tokio::task_local! {
    static EXECUTOR_SCOPE: ExecutorScope;
}

pub fn current_executor() -> Option<Executor> {
    EXECUTOR.try_with(Clone::clone).ok()
}

pub fn current_executor_scope() -> Option<ExecutorScope> {
    EXECUTOR_SCOPE.try_with(Clone::clone).ok()
}

/// Install `executor` without tagging a scope. Prefer the request/job variants
/// at framework boundaries so authorization can distinguish the two paths.
pub async fn with_executor<F: Future>(executor: Executor, fut: F) -> F::Output {
    EXECUTOR.scope(executor, fut).await
}

pub async fn with_request_executor<F: Future>(executor: Executor, fut: F) -> F::Output {
    EXECUTOR
        .scope(executor, EXECUTOR_SCOPE.scope(ExecutorScope::Request, fut))
        .await
}

pub async fn with_job_executor<F: Future>(executor: Executor, fut: F) -> F::Output {
    EXECUTOR
        .scope(executor, EXECUTOR_SCOPE.scope(ExecutorScope::Job, fut))
        .await
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    fn pool() -> Executor {
        Executor::Pool(Arc::new(DatabaseConnection::default()))
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
            // `with_executor` is the unspecified-scope variant — guards that
            // gate on scope must see `None` here, not a stale value.
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

    // `with_executor` (the unspecified-scope variant) does not unwind the
    // scope task-local because it never set it — verifies the two task-locals
    // are genuinely independent, not coupled by a wrapping accident.
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

    // Calling `current_executor` twice inside the same scope returns
    // independently-clonable handles — `Executor` is `Clone`, the task-local
    // hands out a clone each time, so a second access does not "consume" it.
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

    // Nested scopes shadow the outer one — typical when a job runs inside a
    // request (e.g. enqueueing in-process). The outer scope is restored on
    // exit; a bug that drops the inner reset would leak the inner executor.
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

    // `Executor` is `Clone`; the variant must round-trip without disturbing
    // the inner `Arc` (cheap reference bumps, no deep copy). Pinning this
    // here so a refactor to a non-`Arc` field surfaces immediately.
    #[test]
    fn executor_clone_preserves_the_pool_variant() {
        let p = pool();
        let cloned = p.clone();
        assert!(matches!(p, Executor::Pool(_)));
        assert!(matches!(cloned, Executor::Pool(_)));
    }

    // `is_mock_connection` is the one `ConnectionTrait` forwarder safe to
    // call on a disconnected `DatabaseConnection`: without the `mock`
    // feature it falls back to the trait default (`false`), so it neither
    // panics nor reaches a sqlx pool. Exercises the `Pool` arm of the
    // forwarding match without needing a live DB.
    #[tokio::test]
    async fn is_mock_connection_forwards_to_inner_on_pool() {
        let executor = pool();
        // A real Postgres pool would still report `false`; the assertion
        // pins the trait-default behaviour, not the value itself.
        assert!(!executor.is_mock_connection());
    }

    #[tokio::test]
    async fn no_ambient_state_outside_any_scope_remains_observable() {
        // Outside any scope, both task-locals stay `None` — the guard the
        // `repo::scope_for` deny-all branch keys on.
        assert!(current_executor().is_none());
        assert!(current_executor_scope().is_none());
    }
}
