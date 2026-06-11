//! WebSocket data-layer binding (feature `ws`).
//!
//! The gateway's connection loop runs after the upgrade unwinds, so the ORM
//! executor and authz ability the HTTP request installed are gone by the time
//! a message handler runs. This implements `nestrs-ws`'s [`SocketContext`] seam
//! to re-install both around each dispatch. The connection-level guards
//! (`AuthGuard` + `AbilityGuard`) attach the ability to the upgrade request;
//! this bridge captures it once and re-installs it per message — it does **not**
//! re-run the guard chain, unlike the GraphQL bridge.
//!
//! The captured executor is the **pool**: a WebSocket message has no
//! safe/mutating HTTP method to classify, so there is no per-message
//! transaction. Mutating handlers auto-commit individually.

use std::sync::Arc;

use nest_rs_authz::{Ability, with_ability};
use nest_rs_core::injectable;
use nest_rs_ws::{BoxFuture, Captured, SocketContext, WsReply};
use poem::Request;
use sea_orm::DatabaseConnection;

use crate::{Executor, with_request_executor};

/// Captured on upgrade. A guest connection has no `Ability` and its handlers
/// run unscoped — the resolvers' own gate then refuses what it must.
struct CapturedContext {
    executor: Executor,
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
            executor: Executor::Pool((*self.db).clone()),
            ability: req.extensions().get::<Arc<Ability>>().cloned(),
        })
    }

    fn around<'a>(
        &'a self,
        captured: &'a Captured,
        inner: BoxFuture<'a, WsReply>,
    ) -> BoxFuture<'a, WsReply> {
        Box::pin(async move {
            // A downcast miss is a framework bug; run unscoped rather than panic.
            let Some(cx) = captured.downcast_ref::<CapturedContext>() else {
                tracing::error!(
                    target: "nest_rs::ws",
                    reason = "socket_context_downcast_miss",
                    "unexpected captured socket context"
                );
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
