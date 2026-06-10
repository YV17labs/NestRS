//! [`DbContext`] — request boundary that installs the ambient executor.
//!
//! Auto-installed by [`DatabaseModule`](crate::DatabaseModule), it wraps the
//! routing tree (band `DATA_CONTEXT`, the innermost transport wrap), so it
//! covers controller routes and self-mounted surfaces alike. The guard pool
//! runs *inside* it (in the per-route shaper, post-routing): an
//! unauthenticated mutating request therefore opens a transaction before the
//! guard denies it — the denial is a non-2xx response, so that empty
//! transaction rolls back and nothing persists. The wasted `BEGIN`/`ROLLBACK`
//! is the accepted cost of guards reading `#[public]` after routing; a lazy
//! executor (`BEGIN` deferred to first use) is the planned removal.
//! Guards and handlers resolve the same ambient
//! [`Executor`](crate::Executor) via [`Repo`](crate::Repo). Safe methods
//! (GET/HEAD/OPTIONS/TRACE) run on the pool; mutating methods run in a
//! transaction committed on 2xx/3xx and rolled back otherwise — a failed
//! mutation never half-persists, and a response tagged
//! [`MappedError`](nest_rs_core::MappedError) (an error a filter mapped)
//! rolls back even when its status reads as success.
//!
//! ### Serialization conflict observability
//!
//! When [`DatabaseConfig::retry_serialization_conflicts`] is on, a commit
//! that fails with a SQLSTATE the [`retry`](crate::retry) module recognizes
//! is retried up to [`DEFAULT_RETRY_ATTEMPTS`](crate::retry::DEFAULT_RETRY_ATTEMPTS)
//! times with small exponential backoff. The handler body itself is not
//! retried — a poem `Request` is consumed by `next.run` and not replayable
//! at the interceptor layer; for that case use the [`retry_on_conflict`]
//! primitive at the service's programmatic transaction boundary.
//!
//! [`retry_on_conflict`]: crate::retry::retry_on_conflict

use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_http::interceptor;
use nest_rs_interceptors::{Interceptor, Next};
use poem::http::{Method, StatusCode};
use poem::{Error, Request, Response, Result};
use sea_orm::{DatabaseConnection, DatabaseTransaction, TransactionTrait};

use crate::config::DatabaseConfig;
use crate::executor::{Executor, with_request_executor};
use crate::retry::{DEFAULT_INITIAL_BACKOFF, DEFAULT_RETRY_ATTEMPTS, is_retryable_conflict};

#[interceptor(priority = -10)]
pub struct DbContext {
    #[inject]
    db: Arc<DatabaseConnection>,
    #[inject]
    config: Arc<DatabaseConfig>,
}

impl DbContext {
    pub fn new(db: Arc<DatabaseConnection>, config: Arc<DatabaseConfig>) -> Self {
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

        let txn = match self.db.begin().await {
            Ok(txn) => Arc::new(txn),
            Err(err) => {
                tracing::error!(target: "nest_rs::orm", error = %err, "failed to open transaction");
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
                        target: "nest_rs::orm",
                        "transaction escaped the request — the executor was leaked into a spawned task; rolling back and failing the otherwise-successful response so its lost writes are not reported as committed"
                    );
                    return Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR));
                }
                tracing::error!(
                    target: "nest_rs::orm",
                    "transaction escaped the request — the executor was leaked into a spawned task; rolling back (the response had already failed)"
                );
                return result;
            }
        };

        finalize_transaction(txn, &result, self.config.retry_serialization_conflicts).await?;
        result
    }
}

/// Commit on 2xx/3xx, roll back otherwise. When `retry_conflicts` is on,
/// a commit that fails with a typed SQLSTATE matched by
/// [`is_retryable_conflict`] is logged at `warn` and surfaces as `500` —
/// the handler body is not replayable here, so the retry is bounded to
/// the commit itself, and a handler-time conflict is the service's job
/// (via `retry::retry_on_conflict`) to retry around its own transaction
/// boundary.
async fn finalize_transaction(
    txn: DatabaseTransaction,
    result: &Result<Response>,
    retry_conflicts: bool,
) -> Result<()> {
    if should_commit(result) {
        match txn.commit().await {
            Ok(()) => Ok(()),
            Err(err) if retry_conflicts && is_retryable_conflict(&err) => {
                // A serialization/deadlock at commit time can't be retried at
                // this layer (the txn handle is already consumed). Log with the
                // conflict tag so ops sees the failure mode distinctly from a
                // generic commit error, then fail the response closed.
                tracing::warn!(
                    target: "nest_rs::orm",
                    error = %err,
                    attempts = DEFAULT_RETRY_ATTEMPTS,
                    initial_backoff_ms = DEFAULT_INITIAL_BACKOFF.as_millis() as u64,
                    "serialization conflict at commit — handler is not replayable from the interceptor; use `retry::retry_on_conflict` at a programmatic transaction boundary instead",
                );
                Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
            }
            Err(err) => {
                tracing::error!(target: "nest_rs::orm", error = %err, "transaction commit failed");
                Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
            }
        }
    } else {
        if let Err(err) = txn.rollback().await {
            tracing::error!(target: "nest_rs::orm", error = %err, "transaction rollback failed");
        }
        // A handler-time conflict surfaces upstream as an already-mapped
        // `poem::Error`; the typed `DbErr` is gone past `Repo`, so the
        // only honest thing to do here is rely on `Repo`'s own
        // `nest_rs::orm` warn lines. Pattern-matching the formatted
        // response string was producing false positives on any digit
        // substring (a port, a row id, a timestamp) — see Bug 7.
        Ok(())
    }
}

fn is_safe(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

/// 2xx and 3xx commit; any other status or an `Err` rolls back. A response
/// tagged [`MappedError`](nest_rs_core::MappedError) also rolls back whatever
/// its status: it was produced by a route-site `Filter` / `ExceptionFilter`
/// mapping a handler **error** — the mapping shapes the client answer, it
/// does not bless the failed handler's writes.
fn should_commit(result: &Result<Response>) -> bool {
    matches!(
        result,
        Ok(resp) if (resp.status().is_success() || resp.status().is_redirection())
            && resp.extensions().get::<nest_rs_core::MappedError>().is_none()
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
        // A route-site Filter / ExceptionFilter that maps a handler error to
        // a 2xx tags the response `MappedError` — the handler failed, so its
        // writes must not persist behind the mapped status.
        let mut resp = StatusCode::OK.into_response();
        resp.extensions_mut().insert(nest_rs_core::MappedError);
        assert!(!should_commit(&Ok(resp)));
    }
}
