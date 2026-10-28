use std::sync::Arc;

use nest_rs::core::{ambient, error_message};
use nest_rs::ws::{Opaque, Scoped, WsClient, WsError, gateway, messages};

use super::guard::ModerationGuard;
use super::request_seq::RequestSeq;
use crate::Claims;
use crate::authn::AuthnGuard;
use crate::chat::dtos::{ChatMessageDto, SendMessageDto};
use crate::chat::service::ChatService;

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
    #[public]
    #[use_guards(ModerationGuard)]
    async fn on_message(&self, message: SendMessageDto) -> Result<(), WsError> {
        let caller = Self::caller()?;
        self.svc.record(caller.sub.clone(), message).opaque()?;
        Ok(())
    }

    #[subscribe_message("history")]
    #[public]
    async fn history(&self) -> Vec<ChatMessageDto> {
        self.svc.history()
    }

    #[subscribe_message("presence")]
    #[public]
    async fn presence(&self) -> usize {
        self.svc.present()
    }

    #[subscribe_message("seq")]
    #[public]
    async fn seq(&self, seq: Scoped<RequestSeq>) -> u64 {
        seq.value()
    }

    #[subscribe_message("typing")]
    #[public]
    #[use_guards(ModerationGuard)]
    async fn typing(&self, message: SendMessageDto, client: &WsClient) -> Result<(), WsError> {
        let typing = ChatMessageDto {
            author: Self::caller()?.sub.clone(),
            text: message.text,
        };
        if let Err(e) = client.broadcast("typing", &typing) {
            tracing::warn!(target: crate::chat::TARGET, error = %error_message(&e), "broadcast failed");
        }
        Ok(())
    }
}

impl ChatGateway {
    fn caller() -> Result<Arc<Claims>, WsError> {
        ambient::<Claims>()
            .ok_or("no principal reached the message: AuthnGuard did not run at the upgrade")
            .opaque()
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nest_rs::core::Discoverable;

    use super::ChatGateway;
    use crate::chat::service::ChatService;

    #[test]
    fn gateway_declares_its_injected_dependency_for_the_access_graph() {
        assert!(ChatGateway::dependencies().is_empty());
        assert!(ChatGateway::injected().contains(&TypeId::of::<ChatService>()));
    }
}
