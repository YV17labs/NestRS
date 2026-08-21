use nest_rs::ws::{WsClient, gateway, messages};

use features::app_authn::AppAuthnGuard;

pub struct NotifyNs;

#[gateway(path = "/notify", namespace = NotifyNs)]
#[use_guards(AppAuthnGuard)]
#[derive(Default)]
pub struct NotifyGateway {}

#[messages]
impl NotifyGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self, client: &WsClient) {
        if let Err(e) = client.broadcast("pong", &"hi") {
            tracing::warn!(target: "live::notify", error = %e, "broadcast failed");
        }
    }
}
