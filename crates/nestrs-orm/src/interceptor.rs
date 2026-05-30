//! [`DbContext`] — the request boundary that installs the ambient executor.
//!
//! Auto-installed by [`DatabaseModule`](crate::DatabaseModule), it wraps every
//! request *outside* the route's guards, so guards and handlers alike resolve the
//! same ambient [`Executor`](crate::Executor) via [`Repo`](crate::Repo). A **safe**
//! method (GET/HEAD/OPTIONS/TRACE) runs on the pool; a **mutating** method runs in
//! a transaction opened here, committed when the handler answers with a success
//! (2xx) or redirect (3xx) and rolled back otherwise — so a developer never writes
//! a transaction by hand, and a failed mutation never half-persists.

use std::sync::Arc;

use async_trait::async_trait;
use nestrs_http::interceptor;
use nestrs_middleware::{Interceptor, Next};
use poem::http::{Method, StatusCode};
use poem::{Error, Request, Response, Result};
use sea_orm::{DatabaseConnection, TransactionTrait};

use crate::executor::{with_executor, Executor};

#[interceptor]
pub(crate) struct DbContext {
    #[inject]
    db: Arc<DatabaseConnection>,
}

#[async_trait]
impl Interceptor for DbContext {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        if is_safe(req.method()) {
            return with_executor(Executor::Pool(self.db.clone()), next.run(req)).await;
        }

        let txn = match self.db.begin().await {
            Ok(txn) => Arc::new(txn),
            Err(err) => {
                tracing::error!(target: "nestrs::orm", error = %err, "failed to open transaction");
                return Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR));
            }
        };

        let result = with_executor(Executor::Txn(txn.clone()), next.run(req)).await;

        // The handler future has completed and its executor clone dropped, so we
        // are normally the sole owner and can take the transaction back to commit
        // or roll it back. A lingering reference means the executor *escaped* the
        // request — a service leaked it into a spawned task that outlived the
        // handler. We can no longer commit (the leaked task's eventual `Drop` will
        // roll back), so the request's writes are lost. Drop our clone to free the
        // transaction and fail *loudly*: a 2xx/3xx answer would otherwise report
        // success for nothing persisted — silent data loss. A response that already
        // failed keeps its status; its rollback matches its intent.
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
        } else if let Err(err) = txn.rollback().await {
            tracing::error!(target: "nestrs::orm", error = %err, "transaction rollback failed");
        }
        result
    }
}

/// HTTP methods that must not mutate state, so they need no transaction.
fn is_safe(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

/// Whether a handler's outcome should commit its transaction: a success (2xx) or
/// redirect (3xx) response commits; any other status, or an `Err`, rolls back.
fn should_commit(result: &Result<Response>) -> bool {
    matches!(result, Ok(resp) if resp.status().is_success() || resp.status().is_redirection())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use nestrs_middleware::EndpointExt;
    use poem::endpoint::make;
    use poem::{Endpoint, IntoResponse};
    use sea_orm::Database;

    use super::*;
    use crate::executor::current_executor;

    async fn db() -> Arc<DatabaseConnection> {
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must point at a reachable Postgres for this test");
        Arc::new(Database::connect(&url).await.expect("connect to Postgres"))
    }

    fn mutating_request() -> Request {
        Request::builder()
            .method(Method::POST)
            .uri("/".parse().unwrap())
            .finish()
    }

    fn status_of(result: Result<Response>) -> StatusCode {
        match result {
            Ok(resp) => resp.status(),
            Err(err) => err.into_response().status(),
        }
    }

    // A mutating handler that *leaks* the ambient executor into a detached task
    // outliving the request: the transaction the interceptor opened is still
    // referenced when the handler returns, so it can never commit. The framework
    // must turn the handler's 200 into a loud 500 rather than report success for
    // writes that silently roll back.
    #[tokio::test]
    async fn an_escaped_transaction_fails_an_otherwise_successful_response() {
        let ctx = DbContext { db: db().await };

        let endpoint = make(|_req: Request| async {
            let escaped = current_executor().expect("the handler runs with an ambient executor");
            // Leak the executor into a detached task that outlives this handler's
            // return, so a clone of the transaction is still alive when the
            // interceptor reclaims it.
            tokio::spawn(async move {
                let _hold = escaped;
                tokio::time::sleep(Duration::from_secs(30)).await;
            });
            StatusCode::OK.into_response()
        });

        let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
        assert_eq!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "a leaked transaction must surface as a 500, never a false 2xx",
        );
    }

    // The control: a well-behaved mutating handler keeps its own status — the
    // escape detection does not interfere with the normal commit path.
    #[tokio::test]
    async fn a_well_behaved_mutating_handler_keeps_its_status() {
        let ctx = DbContext { db: db().await };

        let endpoint = make(|_req: Request| async {
            current_executor().expect("the handler runs with an ambient executor");
            StatusCode::CREATED.into_response()
        });

        let status = status_of(endpoint.interceptor(ctx).call(mutating_request()).await);
        assert_eq!(status, StatusCode::CREATED);
    }
}
