//! Safe or mutating is read off the HTTP method alone; `/graphql` steps a
//! query-only batch back onto the pool through `Executor::non_transactional`.

use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_http::interceptor;
use nest_rs_interceptors::{Interceptor, Next};
use poem::http::{Method, StatusCode};
use poem::{Error, Request, Response, Result};
use sea_orm::DatabaseConnection;

use crate::SeaOrmConfig;
use crate::error::CommitError;
use crate::executor::{Executor, FinalizeOutcome, LazyTransaction, with_request_executor};

/// The request interceptor that installs the ambient [`Executor`] — the pool for
/// a safe method, a **lazily opened** per-request transaction (committed on
/// 2xx/3xx, rolled back otherwise; never opened when nothing touches the data
/// layer) for a mutating one. Auto-mounted at band −10 by importing
/// [`SeaOrmDatabaseModule`](crate::SeaOrmDatabaseModule).
#[interceptor(priority = -10)]
pub struct DbContext {
    #[inject]
    db: Arc<DatabaseConnection>,
    #[inject]
    config: Arc<SeaOrmConfig>,
}

impl DbContext {
    /// Construct the interceptor from a pool and config.
    pub fn new(db: Arc<DatabaseConnection>, config: Arc<SeaOrmConfig>) -> Self {
        Self { db, config }
    }
}

impl Layer for DbContext {}

#[async_trait]
impl Interceptor for DbContext {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        if is_safe(req.method()) {
            return with_request_executor(Executor::Pool((*self.db).clone()), next.run(req)).await;
        }

        let lazy = Arc::new(LazyTransaction::new((*self.db).clone(), "http"));

        let result = with_request_executor(Executor::Lazy(lazy.clone()), next.run(req)).await;

        let success = should_commit(&result);
        match lazy.finalize(success).await {
            FinalizeOutcome::NoTransaction
            | FinalizeOutcome::Committed
            | FinalizeOutcome::RolledBack => result,
            // A success here would report writes that were never committed.
            FinalizeOutcome::Escaped => {
                if success {
                    Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
                } else {
                    result
                }
            }
            FinalizeOutcome::CommitFailed(err) => Err(commit_failure(
                err,
                self.config.observe_serialization_conflicts,
            )),
            // The handler answered 2xx over a failed statement: its writes are lost.
            FinalizeOutcome::Poisoned { .. } => {
                Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
            }
        }
    }
}

/// Never retried: `next.run` consumed the request, so the handler is not
/// replayable here.
fn commit_failure(err: CommitError, observe_conflicts: bool) -> Error {
    if observe_conflicts && err.is_retryable_conflict() {
        tracing::warn!(
            target: crate::target::ORM,
            error = %nest_rs_core::error_message(&err),
            hint = "not retried here (handler is not replayable from the interceptor); \
                    use `retry::retry_on_conflict` at a programmatic transaction boundary",
            "serialization conflict at commit",
        );
    } else {
        tracing::error!(target: crate::target::ORM, error = %nest_rs_core::error_message(&err), "transaction commit failed");
    }
    Error::from_status(StatusCode::INTERNAL_SERVER_ERROR)
}

fn is_safe(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

/// 2xx and 3xx commit, unless tagged [`MappedError`](nest_rs_http::MappedError):
/// a filter mapping a handler error does not bless its writes.
fn should_commit(result: &Result<Response>) -> bool {
    matches!(
        result,
        Ok(resp) if (resp.status().is_success() || resp.status().is_redirection())
            && resp.extensions().get::<nest_rs_http::MappedError>().is_none()
    )
}

#[cfg(test)]
mod tests {
    use poem::IntoResponse;

    use super::*;

    #[test]
    fn safe_methods_skip_the_transaction_wrapper() {
        assert!(is_safe(&Method::GET));
        assert!(is_safe(&Method::HEAD));
        assert!(is_safe(&Method::OPTIONS));
        assert!(is_safe(&Method::TRACE));
    }

    #[test]
    fn mutating_methods_open_a_transaction() {
        assert!(!is_safe(&Method::POST));
        assert!(!is_safe(&Method::PUT));
        assert!(!is_safe(&Method::PATCH));
        assert!(!is_safe(&Method::DELETE));
    }

    fn response_with(status: StatusCode) -> Result<Response> {
        Ok(status.into_response())
    }

    #[test]
    fn two_xx_commits() {
        assert!(should_commit(&response_with(StatusCode::OK)));
        assert!(should_commit(&response_with(StatusCode::CREATED)));
        assert!(should_commit(&response_with(StatusCode::NO_CONTENT)));
    }

    #[test]
    fn three_xx_commits() {
        assert!(should_commit(&response_with(StatusCode::MOVED_PERMANENTLY)));
        assert!(should_commit(&response_with(StatusCode::SEE_OTHER)));
    }

    #[test]
    fn four_xx_and_five_xx_roll_back() {
        assert!(!should_commit(&response_with(StatusCode::BAD_REQUEST)));
        assert!(!should_commit(&response_with(StatusCode::FORBIDDEN)));
        assert!(!should_commit(&response_with(
            StatusCode::INTERNAL_SERVER_ERROR,
        )));
    }

    #[test]
    fn err_rolls_back() {
        let err: Result<Response> = Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(!should_commit(&err));
    }

    #[test]
    fn a_mapped_error_rolls_back_even_with_a_success_status() {
        let mut resp = StatusCode::OK.into_response();
        resp.extensions_mut().insert(nest_rs_http::MappedError);
        assert!(!should_commit(&Ok(resp)));
    }
}
