//! [`DbContext`] — request boundary that installs the ambient executor.
//!
//! Auto-installed by [`DatabaseModule`](crate::DatabaseModule), it wraps every
//! request *outside* the route's guards, so guards and handlers resolve the same
//! ambient [`Executor`](crate::Executor) via [`Repo`](crate::Repo). Safe methods
//! (GET/HEAD/OPTIONS/TRACE) run on the pool; mutating methods run in a
//! transaction committed on 2xx/3xx and rolled back otherwise — a failed
//! mutation never half-persists.

use std::sync::Arc;

use async_trait::async_trait;
use nestrs_http::interceptor;
use nestrs_middleware::{Interceptor, Next};
use poem::http::{Method, StatusCode};
use poem::{Error, Request, Response, Result};
use sea_orm::{DatabaseConnection, TransactionTrait};

use crate::executor::{Executor, with_request_executor};

#[interceptor]
pub struct DbContext {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl DbContext {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl Interceptor for DbContext {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        if is_safe(req.method()) {
            return with_request_executor(Executor::Pool(self.db.clone()), next.run(req)).await;
        }

        let txn = match self.db.begin().await {
            Ok(txn) => Arc::new(txn),
            Err(err) => {
                tracing::error!(target: "nestrs::orm", error = %err, "failed to open transaction");
                return Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR));
            }
        };

        let result = with_request_executor(Executor::Txn(txn.clone()), next.run(req)).await;

        // A lingering Arc means the executor escaped into a task outliving the
        // handler — we can't commit (the leaked task's eventual Drop rolls back),
        // so an otherwise-successful response is silent data loss. Fail it loud.
        let txn = match Arc::try_unwrap(txn) {
            Ok(txn) => txn,
            Err(escaped) => {
                drop(escaped);
                if should_commit(&result) {
                    tracing::error!(
                        target: "nestrs::orm",
                        "transaction escaped the request — the executor was leaked into a spawned task; rolling back and failing the otherwise-successful response so its lost writes are not reported as committed"
                    );
                    return Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR));
                }
                tracing::error!(
                    target: "nestrs::orm",
                    "transaction escaped the request — the executor was leaked into a spawned task; rolling back (the response had already failed)"
                );
                return result;
            }
        };

        if should_commit(&result) {
            if let Err(err) = txn.commit().await {
                tracing::error!(target: "nestrs::orm", error = %err, "transaction commit failed");
                return Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR));
            }
        } else {
            if let Err(err) = txn.rollback().await {
                tracing::error!(target: "nestrs::orm", error = %err, "transaction rollback failed");
            }
        }
        result
    }
}

fn is_safe(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

/// 2xx and 3xx commit; any other status or an `Err` rolls back.
fn should_commit(result: &Result<Response>) -> bool {
    matches!(result, Ok(resp) if resp.status().is_success() || resp.status().is_redirection())
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
}
