use nest_rs::core::module;
use nest_rs::ws::WsModule;

use super::service::ChatService;

#[module(imports = [WsModule], providers = [ChatService])]
pub struct ChatModule;
