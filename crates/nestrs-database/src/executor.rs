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
