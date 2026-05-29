use std::sync::Arc;

use nestrs_ws::{gateway, messages, WsClient};

use crate::chat::dto::{ChatMessage, SendMessage};
use crate::chat::service::RoomService;

#[gateway(path = "/ws")]
pub struct ChatGateway {
    #[inject]
    room: Arc<RoomService>,
}

#[messages]
impl ChatGateway {
    #[subscribe_message("message")]
    async fn on_message(&self, message: SendMessage) {
        self.room.record(message);
    }

    #[subscribe_message("history")]
    async fn history(&self) -> Vec<ChatMessage> {
        self.room.history()
    }

    #[subscribe_message("typing")]
    async fn typing(&self, message: SendMessage, client: &WsClient) {
        let _ = client.broadcast("typing", &message);
    }
}

#[cfg(test)]
mod tests {
    use std::any::TypeId;

    use nestrs_core::Discoverable;

    use super::ChatGateway;
    use crate::chat::service::RoomService;

    #[test]
    fn gateway_declares_its_injected_dependency_for_the_access_graph() {
        assert!(ChatGateway::dependencies().is_empty());
        assert!(ChatGateway::injected().contains(&TypeId::of::<RoomService>()));
    }
}
