use async_trait::async_trait;
use nestrs_authz::current_ability;
use nestrs_core::injectable;
use nestrs_ws::{MessageGuard, WsClient};

/// Access-graph marker + runtime fail-closed check: rejects the message when
/// no ambient ability is installed, preventing the "no-ability ⇒ unscoped
/// read" footgun the data layer allows by design when a gateway is
/// misconfigured (no connection-level auth, or no `WsDataContext` bound).
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
