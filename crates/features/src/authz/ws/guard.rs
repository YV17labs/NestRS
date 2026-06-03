use async_trait::async_trait;
use nestrs_authz::current_ability;
use nestrs_core::injectable;
use nestrs_ws::{MessageGuard, WsClient};

/// The WebSocket counterpart of HTTP's `#[use_guards(AuthGuard, AppAbilityGuard)]`
/// and GraphQL's `#[use_guards(GraphqlAuthGuard)]`.
///
/// Two jobs, one declarative seam:
///
/// 1. **Access-graph marker.** Bound on a `#[subscribe_message]`, it makes a
///    feature's `<Feature>WsModule` declare `AuthzWsModule` as a dep — without
///    it, the per-message `dyn SocketContext`
///    ([`WsDataContext`](nestrs_database::ws::WsDataContext)) that installs the
///    ambient executor + ability is invisible to the import contract and a
///    handler reaching `Repo` would silently run unscoped (or fail) at runtime.
/// 2. **Runtime fail-closed check.** Asserts the connection's authz state is
///    actually installed before the handler runs. `nestrs-ws` runs message
///    guards **inside** [`SocketContext::around`], so when the gateway's
///    connection-level [`AuthGuard`](crate::authn::AuthGuard) +
///    [`AppAbilityGuard`](crate::authz::AppAbilityGuard) authenticated the
///    upgrade, [`current_ability`] returns the captured ability here. A
///    misconfigured gateway (no connection-level auth, or no `WsDataContext`
///    bound) leaves it unset — the marker then rejects, preventing the
///    "no-ability ⇒ unscoped read" footgun the data layer otherwise allows by
///    design.
#[injectable]
#[derive(Default)]
pub struct WsAuthGuard;

#[async_trait]
impl MessageGuard for WsAuthGuard {
    async fn can_activate(
        &self,
        _client: &WsClient,
        _event: &str,
        _data: &serde_json::Value,
    ) -> Result<(), String> {
        if current_ability().is_none() {
            tracing::warn!(
                target: "nestrs::authz",
                "ws message rejected: no ambient ability — bind connection-level \
                 AuthGuard + AppAbilityGuard on the gateway and \
                 WsDataContext as dyn SocketContext on its module",
            );
            return Err("unauthenticated".into());
        }
        Ok(())
    }
}
