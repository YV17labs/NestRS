//! WebSocket transport binding for the data layer. Enabled by the `ws` Cargo feature.
//!
//! Binds the transparent data layer to the WebSocket surface, the data-side
//! analog of `nestrs_authz::http`'s `Authorize` shaper. A gateway's connection
//! loop runs in a task *after* the upgrade request completes, so the ORM
//! executor and the authz ability the HTTP request installed have unwound by
//! the time a message handler runs. This module implements `nestrs-ws`'s
//! [`SocketContext`] seam to re-install both around each dispatch, so a socket
//! handler's `Repo` reads run against the request executor and scope to the
//! caller's `Ability` exactly like a controller.
//!
//! Bind it on a gateway's app — import `WsModule`, list the bridge `as dyn
//! SocketContext`, and put the connection guards that build the ability
//! (`AuthGuard` + `AbilityGuard`) on the gateway struct:
//!
//! ```ignore
//! #[gateway(path = "/ws")]
//! #[use_guards(AuthGuard, AppAbilityGuard)]
//! struct UsersGateway { #[inject] users: Arc<UsersService> }
//!
//! #[module(
//!     imports = [WsModule, AuthnModule, AuthzModule, UsersModule],
//!     providers = [UsersGateway, WsDataContext as dyn SocketContext],
//! )]
//! struct UsersWsModule;
//! ```
//!
//! Unlike `nestrs_authz::graphql::GraphqlAbilityBridge`, the bridge does **not**
//! re-run the guard chain: the gateway's connection-level guards already
//! authenticated the handshake and attached the `Ability` to the upgrade
//! request, so the bridge only *captures* it (and the executor) and re-installs
//! it per message. It is therefore not generic over the app's guards.
//!
//! **Scope of this cut.** The executor is bound as the connection **pool**, so
//! every message runs on the pool — there is no per-message transaction (a
//! WebSocket message has no safe/mutating HTTP method to classify). A mutating
//! handler's writes auto-commit individually. Per-message transactions, if
//! wanted, would layer on the same seam.

use std::sync::Arc;

use nestrs_authz::{with_ability, Ability};
use nestrs_core::injectable;
use nestrs_ws::{BoxFuture, Captured, SocketContext, WsReply};
use poem::Request;
use sea_orm::DatabaseConnection;

use crate::{with_request_executor, Executor};

/// The opaque per-connection state captured on upgrade: the executor every
/// message runs against, and the caller's ability when the connection
/// authenticated (a guest connection has none, and its handlers run unscoped —
/// the resolvers' own gate then refuses what it must).
struct CapturedContext {
    executor: Executor,
    ability: Option<Arc<Ability>>,
}

/// Re-installs the transparent data context for a gateway's message handlers.
/// Inject it generically by listing it `as dyn SocketContext` on a gateway's
/// module. The connection-level guards (`AuthGuard` + `AbilityGuard`) attach
/// the ability to the upgrade request; this bridge captures it and the pool,
/// then scopes each dispatch to both.
#[injectable]
pub struct WsDataContext {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl SocketContext for WsDataContext {
    fn capture(&self, req: &Request) -> Captured {
        Arc::new(CapturedContext {
            // Every message runs on the shared pool — see the crate-level note
            // on why there is no per-message transaction.
            executor: Executor::Pool(self.db.clone()),
            ability: req.extensions().get::<Arc<Ability>>().cloned(),
        })
    }

    fn around<'a>(
        &'a self,
        captured: &'a Captured,
        inner: BoxFuture<'a, WsReply>,
    ) -> BoxFuture<'a, WsReply> {
        Box::pin(async move {
            // `capture` always produces a `CapturedContext`; a mismatch would
            // be a framework bug, so run unscoped rather than panic on a hot
            // path.
            let Some(cx) = captured.downcast_ref::<CapturedContext>() else {
                tracing::error!(target: "nestrs::ws", "unexpected captured socket context");
                return inner.await;
            };
            let executor = cx.executor.clone();
            match &cx.ability {
                Some(ability) => {
                    with_request_executor(executor, with_ability(ability.clone(), inner)).await
                }
                None => with_request_executor(executor, inner).await,
            }
        })
    }
}
