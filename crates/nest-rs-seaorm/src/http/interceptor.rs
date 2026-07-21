//! [`DbContext`] — request boundary that installs the ambient executor.
//!
//! Auto-installed by [`DatabaseModule`](crate::DatabaseModule), it wraps the
//! routing tree (band `DATA_CONTEXT`, the innermost transport wrap), so it
//! covers controller routes and self-mounted surfaces alike. The guard pool
//! runs *inside* it (in the per-route shaper, post-routing). The executor a
//! mutating method gets is **lazy**: `BEGIN` is deferred to the first
//! data-layer touch, so a request a guard denies — or one that never queries
//! — costs no transaction at all (no pool connection, no Postgres transaction
//! slot; an unauthenticated POST flood cannot amplify into `BEGIN`/`ROLLBACK`
//! round-trips). Guards and handlers resolve the same ambient
//! [`Executor`](crate::Executor) via [`Repo`](crate::Repo). Safe methods
//! (GET/HEAD/OPTIONS/TRACE) run on the pool; mutating methods run in a
//! transaction committed on 2xx/3xx and rolled back otherwise — a failed
//! mutation never half-persists, and a response tagged
//! [`MappedError`](nest_rs_core::MappedError) (an error a filter mapped)
//! rolls back even when its status reads as success.
//!
//! The safe/mutating split is the HTTP **method**, which is all this layer
//! knows. A transport that can prove more about the operation steps back out
//! of the transaction itself through
//! [`Executor::non_transactional`](nest_rs_database::Executor::non_transactional):
//! every GraphQL operation is a POST, so the `/graphql` endpoint routes a batch
//! whose every operation parses as a `query` onto the pool for its duration
//! (DATA-S5). The lazy transaction installed here is then never opened and
//! finalizes as a no-op.
//!
//! ### Serialization conflict observability
//!
//! This interceptor does **not** retry — it cannot: a poem `Request` is
//! consumed by `next.run` and is not replayable at this layer. When
//! [`DatabaseConfig::retry_serialization_conflicts`] is on, a commit that fails
//! with a SQLSTATE the [`retry`](crate::retry) module recognizes is merely
//! *tagged* — logged at `warn` as a serialization conflict for observability —
//! then the request still fails closed (`500`). To actually retry a conflicting
//! transaction, wrap the work in the [`retry_on_conflict`] primitive at the
//! service's programmatic transaction boundary (where the body *is* replayable);
//! the knob here only controls whether the conflict is surfaced distinctly in
//! the logs.
//!
//! [`retry_on_conflict`]: crate::retry::retry_on_conflict

use std::sync::Arc;

use async_trait::async_trait;
use nest_rs_core::Layer;
use nest_rs_http::interceptor;
use nest_rs_interceptors::{Interceptor, Next};
use poem::http::{Method, StatusCode};
use poem::{Error, Request, Response, Result};
use sea_orm::DatabaseConnection;

use crate::config::DatabaseConfig;
use crate::executor::{
    CommitError, Executor, FinalizeOutcome, LazyTransaction, with_request_executor,
};

/// The request interceptor that installs the ambient [`Executor`] — the pool for
/// a safe method, a **lazily opened** per-request transaction (committed on
/// 2xx/3xx, rolled back otherwise; never opened when nothing touches the data
/// layer) for a mutating one. Auto-mounted at band −10 by importing
/// [`DatabaseModule`](crate::DatabaseModule).
#[interceptor(priority = -10)]
pub struct DbContext {
    #[inject]
    db: Arc<DatabaseConnection>,
    #[inject]
    config: Arc<DatabaseConfig>,
}

impl DbContext {
    /// Construct the interceptor from a pool and config — the honest constructor
    /// tests use in place of container resolution.
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

        // Lazy: `BEGIN` runs on the first data-layer touch, inside the
        // executor itself. A guard denial (or a handler that never queries)
        // leaves the cell empty and this request costs no transaction.
        let lazy = Arc::new(LazyTransaction::new((*self.db).clone()));

        let result = with_request_executor(Executor::Lazy(lazy.clone()), next.run(req)).await;

        let success = should_commit(&result);
        match lazy.finalize(success, "http").await {
            FinalizeOutcome::NoTransaction
            | FinalizeOutcome::Committed
            | FinalizeOutcome::RolledBack => result,
            // The escape invariant (logged by `finalize`): a handle outliving
            // the handler cannot be committed, so an otherwise-successful
            // response is silent data loss — fail it loud.
            FinalizeOutcome::Escaped { .. } => {
                if success {
                    Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
                } else {
                    result
                }
            }
            FinalizeOutcome::CommitFailed(err) => Err(commit_failure(
                err,
                self.config.retry_serialization_conflicts,
            )),
        }
    }
}

/// Classify a commit-time failure. When `retry_conflicts` is on, a typed
/// SQLSTATE matched by [`CommitError::is_retryable_conflict`] is tagged at
/// `warn` for observability — the interceptor does **not** retry (the handler
/// body is not replayable here; use `retry::retry_on_conflict` at a
/// programmatic transaction boundary); anything else logs at `error`. Either
/// way the response fails closed.
fn commit_failure(err: CommitError, retry_conflicts: bool) -> Error {
    if retry_conflicts && err.is_retryable_conflict() {
        tracing::warn!(
            target: "nest_rs::orm",
            error = %err,
            hint = "not retried here (handler is not replayable from the interceptor); \
                    use `retry::retry_on_conflict` at a programmatic transaction boundary",
            "serialization conflict at commit",
        );
    } else {
        tracing::error!(target: "nest_rs::orm", error = %err, "transaction commit failed");
    }
    Error::from_status(StatusCode::INTERNAL_SERVER_ERROR)
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
