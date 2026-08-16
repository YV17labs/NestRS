//! The one per-dispatch data-context dance, shared by every transport whose
//! handler runs after the HTTP request's task-locals have unwound.
//!
//! `DbContext` installs the executor + ability for an HTTP request, but a WS
//! message loop and an MCP tool call both run on tasks that request never
//! touched. Each transport therefore **captures** the pool + ability while
//! still on the request, then **re-installs** them around its own dispatch.
//! That second half is identical everywhere — same lazy transaction, same
//! commit-on-success / rollback-on-failure, same escaped-handle and
//! commit-failure handling — so it lives here once.
//!
//! Keeping it in one place is not only DRY: a divergence between two
//! transports would mean a transaction bug fixed on one and left standing on
//! the other, which is exactly the class of drift the framework exists to
//! prevent.

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use nest_rs_authz::{Ability, with_ability};
use poem::Request;
use sea_orm::DatabaseConnection;

use crate::executor::{FinalizeOutcome, LazyTransaction};
use crate::{Executor, with_request_executor};

/// What a transport snapshots on the HTTP request to replay later: the pool to
/// open per-dispatch transactions on, and the caller's ability (absent for an
/// unauthenticated caller — `Repo` then denies every row).
pub(crate) struct RequestSnapshot {
    pool: DatabaseConnection,
    ability: Option<Arc<Ability>>,
}

impl RequestSnapshot {
    /// Capture from the post-guard request, while the ability the guard chain
    /// attached is still reachable.
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
/// `captured` is the transport's opaque handle from its own `capture` — the one
/// this module produced. A downcast miss is a framework bug, so `inner` runs
/// bare (no ambient executor ⇒ `Repo::conn()` errors, fail-closed) rather than
/// panicking.
///
/// `succeeded` reads the transport's own outcome type (a `WsReply::Error`, an
/// `Err(McpError)`) to decide commit vs rollback; `internal_error` builds that
/// transport's opaque failure for the two cases where a *successful* handler
/// cannot be honoured — an escaped transaction handle, or a commit that failed.
/// Reporting success after either would silently lose writes.
pub(crate) async fn with_data_context<T>(
    captured: &Arc<dyn Any + Send + Sync>,
    transport: &'static str,
    inner: Pin<Box<dyn Future<Output = T> + Send + '_>>,
    succeeded: fn(&T) -> bool,
    internal_error: fn() -> T,
) -> T {
    let Some(snapshot) = captured.downcast_ref::<RequestSnapshot>() else {
        tracing::error!(
            target: "nest_rs::orm",
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
        // Already logged at `error` by `finalize`: a statement failed inside
        // the transaction while the operation reported success, so its writes
        // are gone. Report the edge's opaque failure rather than the success.
        FinalizeOutcome::Poisoned { .. } => internal_error(),
        FinalizeOutcome::CommitFailed(err) => {
            tracing::error!(
                target: "nest_rs::orm",
                transport = transport,
                error = %err,
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

    /// A `SocketContext`/`McpToolContext` implementation captures per-request
    /// state as an opaque `Arc<dyn Any>` and this seam downcasts it back. A
    /// mismatch is a framework bug — but the framework is not the only author
    /// of those traits, and what it does about the bug decides whether an
    /// operation runs *unscoped*.
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

        // The operation still runs — refusing it would take a whole transport
        // down over a mismatch that `Repo` already answers safely — but it runs
        // with no ambient executor and no ambient ability, which is the
        // fail-closed shape: every scoped read denies.
        assert!(
            outcome,
            "the operation runs, and it runs with nothing installed",
        );

        let event = logs.expect_one("nest_rs::orm", "unexpected captured data context");
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
