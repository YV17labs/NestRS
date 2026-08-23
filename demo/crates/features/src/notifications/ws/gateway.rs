use nest_rs::ws::{WsClient, gateway, messages};

use crate::authn::AuthnGuard;

pub struct NotificationsNs;

#[gateway(path = "/notify", namespace = NotificationsNs)]
#[use_guards(AuthnGuard)]
#[derive(Default)]
pub struct NotificationsGateway;

#[messages]
impl NotificationsGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self, client: &WsClient) {
        if let Err(e) = client.broadcast("pong", &"hi") {
            tracing::warn!(target: "features::notifications", error = %e, "broadcast failed");
        }
    }
}
