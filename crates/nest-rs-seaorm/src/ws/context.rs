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
//! **Per-message transactions, lazily**, through the same
//! [`with_data_context`](crate::dispatch::with_data_context) every other
//! after-the-request transport uses: `BEGIN` is deferred to the handler's first
//! data-layer touch, so a read-only or non-querying message costs no
//! transaction at all, while a writing handler gets the same
//! commit-on-success / rollback-on-error semantics as an HTTP mutation. Success
//! is a [`WsReply::Reply`]/[`WsReply::None`]; a [`WsReply::Error`] rolls back.
//!
//! A guest connection has no `Ability`; `Repo`'s `scope_for` then denies every
//! row on this request-tagged executor (fail-closed) — a handler that must
//! serve guests reads through an explicitly public path, never silently
//! unscoped.

use std::sync::Arc;

use nest_rs_core::injectable;
use nest_rs_ws::{BoxFuture, Captured, SocketContext, WsReply};
use poem::Request;
use sea_orm::DatabaseConnection;

use crate::dispatch::{RequestSnapshot, with_data_context};

/// Re-installs the data context for a gateway's message handlers. List `as dyn
/// SocketContext` on the gateway's module.
#[injectable]
pub struct WsDataContext {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl SocketContext for WsDataContext {
    fn capture(&self, req: &Request) -> Captured {
        Arc::new(RequestSnapshot::capture(&self.db, req))
    }

    fn around<'a>(
        &'a self,
        captured: &'a Captured,
        inner: BoxFuture<'a, WsReply>,
    ) -> BoxFuture<'a, WsReply> {
        Box::pin(with_data_context(
            captured,
            "ws",
            inner,
            |reply| !matches!(reply, WsReply::Error(_)),
            || WsReply::error("internal error"),
        ))
    }
}
