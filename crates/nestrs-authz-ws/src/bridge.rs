//! [`WsDataContext`] — the per-connection bridge that re-installs the request
//! executor and the caller's ambient [`Ability`] around each gateway message,
//! the WebSocket analog of `nestrs-authz-graphql`'s `GraphqlAbilityBridge`. It
//! implements `nestrs-ws`'s [`SocketContext`] seam.

use std::sync::Arc;

use nestrs_authz::{with_ability, Ability};
use nestrs_core::injectable;
use nestrs_database::{with_request_executor, Executor};
use nestrs_ws::{BoxFuture, Captured, SocketContext, WsReply};
use poem::Request;
use sea_orm::DatabaseConnection;

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
/// module. The connection-level guards (`AuthGuard` + `AbilityGuard`) attach the
/// ability to the upgrade request; this bridge captures it and the pool, then
/// scopes each dispatch to both.
#[injectable]
pub struct WsDataContext {
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl SocketContext for WsDataContext {
    fn capture(&self, req: &Request) -> Captured {
        Arc::new(CapturedContext {
            // Every message runs on the shared pool — see the crate-level note on
            // why there is no per-message transaction.
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
            // `capture` always produces a `CapturedContext`; a mismatch would be a
            // framework bug, so run unscoped rather than panic on a hot path.
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
