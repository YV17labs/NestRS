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
pub(crate) struct DbContext {
    #[inject]
    db: Arc<DatabaseConnection>,
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
    use std::time::Duration;

    use nestrs_middleware::EndpointExt;
    use poem::endpoint::make;
    use poem::{Endpoint, IntoResponse};
    use sea_orm::Database;

    use super::*;
    use crate::executor::current_executor;

    async fn db() -> Arc<DatabaseConnection> {
        let url = std::env::var("NESTRS_DATABASE__URL")
            .expect("NESTRS_DATABASE__URL must point at a reachable Postgres for this test");
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

    // Leaking the ambient executor into a detached task that outlives the
    // handler must turn the would-be 200 into a loud 500 — silent rollback of
    // a "successful" mutation is data loss.
    #[tokio::test]
    async fn an_escaped_transaction_fails_an_otherwise_successful_response() {
        let ctx = DbContext { db: db().await };

        let endpoint = make(|_req: Request| async {
            let escaped = current_executor().expect("the handler runs with an ambient executor");
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
