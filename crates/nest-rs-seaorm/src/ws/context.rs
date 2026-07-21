//! WebSocket data-layer binding (feature `ws`).
//!
//! The gateway's connection loop runs after the upgrade unwinds, so the ORM
//! executor and authz ability the HTTP request installed are gone by the time
//! a message handler runs. This implements `nest-rs-ws`'s [`SocketContext`] seam
//! to re-install both around each dispatch. The connection-level guards
//! (`AuthnGuard` + `AbilityGuard`) attach the ability to the upgrade request;
//! this bridge captures it once and re-installs it per message — it does **not**
//! re-run the guard chain, unlike the GraphQL bridge.
//!
//! **The ability is frozen at the upgrade (DATA-S7).** A mid-connection
//! revocation, logout, or token expiry does not propagate to an already-open
//! socket — every message runs under the ability captured at connect. The bound
//! on that stale-privilege window is the socket-lifetime ceiling
//! (`nest_rs_ws::WsConfig::max_connection`, default 4h): when it elapses the
//! server closes the socket, forcing a fresh upgrade and with it a fresh
//! authn/authz + `exp` check. Tightening the ceiling per-connection to the
//! token's own `exp` needs a transport-generic ambient-expiry seam the auth
//! strategy populates — tracked as a post-1.0 enhancement, not a silent gap.
//!
//! **Per-message transactions, lazily.** Each dispatch installs an
//! [`Executor::Lazy`]: `BEGIN` is deferred to the handler's first data-layer
//! touch, so a read-only or non-querying message costs no transaction at all,
//! while a writing handler gets the same commit-on-success /
//! rollback-on-error semantics as an HTTP mutation — a multi-write handler
//! that fails mid-way never half-persists. Success is a
//! [`WsReply::Reply`]/[`WsReply::None`]; a [`WsReply::Error`] rolls back.
//!
//! A guest connection has no `Ability`; `Repo`'s `scope_for` then denies every
//! row on this request-tagged executor (fail-closed) — a handler that must
//! serve guests reads through an explicitly public path, never silently
//! unscoped.

use std::sync::Arc;

use nest_rs_authz::{Ability, with_ability};
use nest_rs_core::injectable;
use nest_rs_ws::{BoxFuture, Captured, SocketContext, WsReply};
use poem::Request;
use sea_orm::DatabaseConnection;

use crate::executor::{FinalizeOutcome, LazyTransaction};
use crate::{Executor, with_request_executor};

/// Captured on upgrade: the pool to open per-message transactions on, and the
/// caller's ability (absent on a guest connection — deny-all under `Repo`).
struct CapturedContext {
    pool: DatabaseConnection,
    ability: Option<Arc<Ability>>,
}

/// Re-installs the data context for a gateway's message handlers. List `as dyn
/// SocketContext` on the gateway's module.
#[injectable]
pub struct WsDataContext {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl SocketContext for WsDataContext {
    fn capture(&self, req: &Request) -> Captured {
        Arc::new(CapturedContext {
            pool: (*self.db).clone(),
            ability: req.extensions().get::<Arc<Ability>>().cloned(),
        })
    }

    fn around<'a>(
        &'a self,
        captured: &'a Captured,
        inner: BoxFuture<'a, WsReply>,
    ) -> BoxFuture<'a, WsReply> {
        Box::pin(async move {
            // A downcast miss is a framework bug; run bare (no ambient
            // executor ⇒ `Repo::conn()` errors, fail-closed) rather than panic.
            let Some(cx) = captured.downcast_ref::<CapturedContext>() else {
                tracing::error!(
                    target: "nest_rs::ws",
                    reason = "socket_context_downcast_miss",
                    "unexpected captured socket context"
                );
                return inner.await;
            };
            let lazy = Arc::new(LazyTransaction::new(cx.pool.clone()));
            let executor = Executor::Lazy(lazy.clone());
            let reply = match &cx.ability {
                Some(ability) => {
                    with_request_executor(executor, with_ability(ability.clone(), inner)).await
                }
                None => with_request_executor(executor, inner).await,
            };
            // Settle the message's lazily opened transaction: commit on a
            // success reply, roll back on an error reply. The escape
            // invariant lives in `finalize`; an escaped handle on a success
            // reply fails loudly rather than silently losing writes.
            let success = !matches!(reply, WsReply::Error(_));
            match lazy.finalize(success, "ws").await {
                FinalizeOutcome::NoTransaction
                | FinalizeOutcome::Committed
                | FinalizeOutcome::RolledBack => reply,
                FinalizeOutcome::Escaped { .. } => {
                    if success {
                        WsReply::error("internal error")
                    } else {
                        reply
                    }
                }
                FinalizeOutcome::CommitFailed(err) => {
                    tracing::error!(
                        target: "nest_rs::orm",
                        transport = "ws",
                        error = %err,
                        "message transaction commit failed"
                    );
                    WsReply::error("internal error")
                }
            }
        })
    }
}
