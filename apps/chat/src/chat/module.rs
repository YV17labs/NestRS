use nestrs_core::module;
use nestrs_ws::WsModule;

use crate::chat::gateway::ChatGateway;
use crate::chat::guard::ModeratedGuard;
use crate::chat::service::RoomService;

#[module(imports = [WsModule], providers = [RoomService, ModeratedGuard, ChatGateway])]
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
