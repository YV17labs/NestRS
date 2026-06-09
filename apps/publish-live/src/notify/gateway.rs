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
        let _ = client.broadcast("pong", &"hi");
    }
}
