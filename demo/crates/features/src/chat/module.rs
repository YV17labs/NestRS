use nest_rs::core::module;
use nest_rs::ws::WsModule;

use super::service::ChatService;

#[module(imports = [WsModule], providers = [ChatService])]
pub struct ChatModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs::core::Container;
    use std::sync::Arc;

    #[test]
    fn registers_chat_service() {
        let container = Container::builder().import::<ChatModule>().build();
        let svc: Option<Arc<ChatService>> = container.get();
        assert!(svc.is_some());
    }
}
