use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_authz::{Ability, with_ability};
use poem::Request;
use sea_orm::DatabaseConnection;

use crate::executor::{FinalizeOutcome, LazyTransaction};
use crate::{Executor, with_request_executor};

/// The pool and the caller's ability, captured on the HTTP request to re-install
/// around a dispatch that runs after its task-locals unwound.
pub(crate) struct RequestSnapshot {
    pool: DatabaseConnection,
    ability: Option<Arc<Ability>>,
}

impl RequestSnapshot {
    /// Capture from the post-guard request, while the guard's ability is reachable.
    pub(crate) fn capture(db: &DatabaseConnection, req: &Request) -> Self {
        Self {
            pool: db.clone(),
            ability: req.extensions().get::<Arc<Ability>>().cloned(),
        }
    }
}

/// Run `inner` with the captured snapshot's executor + ability installed, then
/// settle the lazily opened transaction.
///
/// A downcast miss runs `inner` bare, fail-closed: no ambient executor, so
/// `Repo::conn()` errors. `internal_error` replaces a success whose writes could
/// not land (an escaped handle, a poisoned or failed commit).
pub(crate) async fn with_data_context<T>(
    captured: &Arc<dyn Any + Send + Sync>,
    transport: &'static str,
    inner: Pin<Box<dyn Future<Output = T> + Send + '_>>,
    succeeded: fn(&T) -> bool,
    internal_error: fn() -> T,
) -> T {
    let Some(snapshot) = captured.downcast_ref::<RequestSnapshot>() else {
        tracing::error!(
            target: crate::target::ORM,
            transport = transport,
            reason = "data_context_downcast_miss",
            "unexpected captured data context",
        );
        return inner.await;
    };
    let lazy = Arc::new(LazyTransaction::new(snapshot.pool.clone(), transport));
    let executor = Executor::Lazy(lazy.clone());

    let outcome = match &snapshot.ability {
        Some(ability) => {
            with_request_executor(executor, with_ability(ability.clone(), inner)).await
        }
        None => with_request_executor(executor, inner).await,
    };

    let success = succeeded(&outcome);
    match lazy.finalize(success).await {
        FinalizeOutcome::NoTransaction
        | FinalizeOutcome::Committed
        | FinalizeOutcome::RolledBack => outcome,
        FinalizeOutcome::Escaped => {
            if success {
                internal_error()
            } else {
                outcome
            }
        }
        FinalizeOutcome::Poisoned { .. } => internal_error(),
        FinalizeOutcome::CommitFailed(err) => {
            tracing::error!(
                target: crate::target::ORM,
                transport = transport,
                error = %nest_rs_core::error_message(&err),
                "dispatch transaction commit failed"
            );
            internal_error()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::current_executor;

    use super::*;

    #[tokio::test]
    async fn a_capture_this_seam_does_not_recognise_runs_the_operation_unscoped_and_says_so() {
        let logs = nest_rs_testing::LogCapture::install();
        let foreign: Arc<dyn Any + Send + Sync> = Arc::new("not a request snapshot");

        let outcome = with_data_context(
            &foreign,
            "ws",
            Box::pin(async { current_executor().is_none() }),
            |ran_bare| *ran_bare,
            || false,
        )
        .await;

        assert!(
            outcome,
            "the operation runs, and it runs with nothing installed",
        );

        let event = logs.expect_one(crate::target::ORM, "unexpected captured data context");
        assert_eq!(event.level, "error");
        assert_eq!(
            event.field("transport").as_deref(),
            Some("ws"),
            "the event names the edge whose context seam is wrong, since one \
             function serves several: {:?}",
            event.fields,
        );
        assert_eq!(
            event.field("reason").as_deref(),
            Some("data_context_downcast_miss"),
            "{:?}",
            event.fields,
        );
    }
}
