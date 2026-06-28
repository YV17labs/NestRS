use features::authn::AuthnModule;
use nest_rs_core::module;
use nest_rs_ws::WsModule;

use crate::chat::gateway::ChatGateway;
use crate::chat::guard::ModeratedGuard;
use crate::chat::service::RoomService;

#[module(imports = [WsModule, AuthnModule], providers = [RoomService, ModeratedGuard, ChatGateway])]
pub struct ChatModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_authn::{JwtOptions, JwtService};
    use nest_rs_core::{Container, Module};
    use std::sync::Arc;

    #[test]
    fn registers_room_service() {
        let jwt = JwtService::new(JwtOptions::new("test-only-hs256-secret-at-least-32-bytes"))
            .expect("32+ byte HS256 secret");
        let container = ChatModule::register(Container::builder().provide(jwt)).build();
        let room: Option<Arc<RoomService>> = container.get();
        assert!(room.is_some());
    }
}
