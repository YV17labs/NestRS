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
    let lazy = Arc::new(LazyTransaction::new(snapshot.pool.clone()));
    let executor = Executor::Lazy(lazy.clone());

    let outcome = match &snapshot.ability {
        Some(ability) => {
            with_request_executor(executor, with_ability(ability.clone(), inner)).await
        }
        None => with_request_executor(executor, inner).await,
    };

    let success = succeeded(&outcome);
    match lazy.finalize(success, transport).await {
        FinalizeOutcome::NoTransaction
        | FinalizeOutcome::Committed
        | FinalizeOutcome::RolledBack => outcome,
        FinalizeOutcome::Escaped { .. } => {
            if success {
                internal_error()
            } else {
                outcome
            }
        }
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
