use std::sync::Arc;

use nest_rs::authz::Create;
use nest_rs::core::{Ambient, Scoped, error_message};
use nest_rs::ws::{Opaque, WsClient, WsError, gateway, messages};

use super::request_seq::RequestSeq;
use crate::Claims;
use crate::authn::AuthnGuard;
use crate::authz::AuthzGuard;
use crate::chat::Posting;
use crate::chat::dtos::{ChatMessageDto, SendMessageDto};
use crate::chat::service::ChatService;

#[gateway(path = "/ws")]
#[use_guards(AuthnGuard, AuthzGuard)]
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
    #[authorize(Create, Posting)]
    async fn on_message(
        &self,
        caller: Ambient<Claims>,
        message: SendMessageDto,
    ) -> Result<(), WsError> {
        self.svc.record(caller.into_inner().sub, message).opaque()?;
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
    async fn seq(&self, seq: Scoped<RequestSeq>) -> u64 {
        seq.value()
    }

    #[subscribe_message("typing")]
    #[authorize(Create, Posting)]
    async fn typing(
        &self,
        caller: Ambient<Claims>,
        message: SendMessageDto,
        client: &WsClient,
    ) -> Result<(), WsError> {
        let typing = ChatMessageDto {
            author: caller.into_inner().sub,
            text: message.text,
        };
        if let Err(e) = client.broadcast("typing", &typing) {
            tracing::warn!(target: crate::chat::TARGET, error = %error_message(&e), "broadcast failed");
        }
        Ok(())
    }
}
