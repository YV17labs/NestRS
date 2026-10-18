use nest_rs::core::module;
use nest_rs::ws::WsModule;

use super::service::ChatService;

#[module(imports = [WsModule], providers = [ChatService])]
pub struct ChatModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs::core::Container;
    use nest_rs::ws::WsConfig;
    use std::sync::Arc;

    #[test]
    fn registers_chat_service() {
        let container = Container::builder()
            .provide(WsConfig::default())
            .import::<ChatModule>()
            .build()
            .expect("the container builds");
        let svc: Option<Arc<ChatService>> = container.get();
        assert!(svc.is_some());
    }
}
