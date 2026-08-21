use features::app_authn::AppAuthnModule;
use nest_rs::core::module;
use nest_rs::ws::WsModule;

use crate::chat::gateway::ChatGateway;
use crate::chat::guard::ModeratedGuard;
use crate::chat::request_seq::{RequestSeq, SeqSource};
use crate::chat::service::ChatService;

#[module(imports = [WsModule, AppAuthnModule], providers = [ChatService, ModeratedGuard, ChatGateway, SeqSource, RequestSeq])]
pub struct ChatModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs::authn::{JwtOptions, JwtService};
    use nest_rs::core::{Container, Module};
    use std::sync::Arc;

    #[test]
    fn registers_chat_service() {
        let jwt = JwtService::new(JwtOptions::new("test-only-hs256-secret-at-least-32-bytes"))
            .expect("32+ byte HS256 secret");
        let container = ChatModule::register(Container::builder().provide(jwt)).build();
        let svc: Option<Arc<ChatService>> = container.get();
        assert!(svc.is_some());
    }
}
