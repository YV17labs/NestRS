use nestrs_ws::{gateway, messages, WsClient};

pub struct NotifyNs;

#[gateway(path = "/notify", namespace = NotifyNs)]
#[derive(Default)]
pub struct NotifyGateway {}

#[messages]
impl NotifyGateway {
    #[subscribe_message("ping")]
    async fn ping(&self, client: &WsClient) {
        let _ = client.broadcast("pong", &"hi");
    }
}
