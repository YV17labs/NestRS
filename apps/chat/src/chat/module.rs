use nestrs_core::module;

use crate::chat::gateway::ChatGateway;
use crate::chat::service::RoomService;

#[module(providers = [RoomService, ChatGateway])]
pub struct ChatModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nestrs_core::{Container, Module};
    use std::sync::Arc;

    #[test]
    fn registers_room_service() {
        let container = ChatModule::register(Container::builder()).build();
        let room: Option<Arc<RoomService>> = container.get();
        assert!(room.is_some());
    }
}
