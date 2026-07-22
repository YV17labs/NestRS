use std::sync::Arc;

use nest_rs_ws::{Scoped, WsClient, WsScopeError, gateway, messages, serde_json};

use crate::chat::dtos::{ChatMessageDto, SendMessageDto};
use crate::chat::guard::ModeratedGuard;
use crate::chat::request_seq::RequestSeq;
use crate::chat::service::ChatService;
use features::authn::AuthnGuard;

#[gateway(path = "/ws")]
#[use_guards(AuthnGuard)]
pub struct ChatGateway {
    #[inject]
    svc: Arc<ChatService>,
}

#[messages]
impl ChatGateway {
    #[on_connect]
    async fn joined(&self, client: &WsClient) {
        client.join("lobby");
        self.svc.connected();
    }

    #[on_disconnect]
    async fn left(&self) {
        self.svc.disconnected();
    }

    #[subscribe_message("message")]
    #[use_guards(ModeratedGuard)]
    async fn on_message(&self, message: SendMessageDto) -> Result<(), serde_json::Error> {
        self.svc.record(message)?;
        Ok(())
    }

    #[subscribe_message("history")]
    async fn history(&self) -> Vec<ChatMessageDto> {
        self.svc.history()
    }

    #[subscribe_message("presence")]
    async fn presence(&self) -> usize {
        self.svc.present()
    }

    #[subscribe_message("seq")]
    async fn seq(&self) -> Result<u64, WsScopeError> {
        Ok(Scoped::<RequestSeq>::from_context()?.value())
    }

    #[subscribe_message("typing")]
    async fn typing(&self, message: SendMessageDto, client: &WsClient) {
        if let Err(e) = client.broadcast("typing", &message) {
            tracing::warn!(target: "live::chat", error = %e, "broadcast failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs_core::Discoverable;

    use super::ChatGateway;
    use crate::chat::service::ChatService;

    #[test]
    fn gateway_declares_its_injected_dependency_for_the_access_graph() {
        assert!(ChatGateway::dependencies().is_empty());
        assert!(ChatGateway::injected().contains(&TypeId::of::<ChatService>()));
    }
}
