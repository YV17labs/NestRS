use nest_rs_ws::{WsClient, gateway, messages};

use features::authn::AuthGuard;

pub struct NotifyNs;

#[gateway(path = "/notify", namespace = NotifyNs)]
#[use_guards(AuthGuard)]
#[derive(Default)]
pub struct NotifyGateway {}

#[messages]
impl NotifyGateway {
    #[subscribe_message("ping")]
    async fn ping(&self, client: &WsClient) {
        if let Err(e) = client.broadcast("pong", &"hi") {
            tracing::warn!(target: "live::notify", error = %e, "broadcast failed");
        }
    }
}
