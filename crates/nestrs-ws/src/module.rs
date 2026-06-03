//! [`WsModule`] — provides the [`WsServer`] connection registry. Import it in
//! any module whose gateways broadcast or whose services push to clients, so
//! `#[inject] Arc<WsServer>` resolves and passes the access graph.

use nestrs_core::module;

use crate::server::WsServer;

#[module(providers = [WsServer])]
pub struct WsModule;

#[cfg(test)]
mod tests {
    use super::*;
    use nestrs_core::{Container, Module};
    use std::sync::Arc;

    #[test]
    fn provides_the_server_registry() {
        let container = WsModule::register(Container::builder()).build();
        let server: Option<Arc<WsServer>> = container.get();
        assert!(server.is_some());
    }
}
