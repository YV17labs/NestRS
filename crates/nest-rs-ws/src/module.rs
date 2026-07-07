//! [`WsModule`] — provides the [`WsServer`] connection registry and resolves
//! [`WsConfig`] (the socket-lifetime ceiling). Import it in any module whose
//! gateways broadcast or whose services push to clients, so `#[inject]
//! Arc<WsServer>` resolves and passes the access graph.
//!
//! [`WsConfig`] loads from `NESTRS_WS__*` by default (importing `WsModule` is
//! enough); pin it in code with [`WsModule::for_root`].

use nest_rs_config::ConfigModule;
use nest_rs_core::{ContainerBuilder, DynamicModule, Module, module};

use crate::config::WsConfig;
use crate::server::WsServer;

#[module(imports = [ConfigModule::for_feature::<WsConfig>()], providers = [WsServer])]
pub struct WsModule;

impl WsModule {
    /// `None` ⇒ load [`WsConfig`] from `NESTRS_WS__*`; `Some(cfg)` pins it in
    /// code. Either way the [`WsServer`] registry is provided, so this is a
    /// drop-in replacement for importing the bare [`WsModule`].
    pub fn for_root(config: impl Into<Option<WsConfig>>) -> WsSetup {
        WsSetup {
            pinned: config.into(),
        }
    }
}

/// [`DynamicModule`] returned by [`WsModule::for_root`]: pins (or env-loads)
/// [`WsConfig`], then brings the base [`WsModule`] wiring (the [`WsServer`]
/// registry). A pinned value is a direct provide, so it wins over — and skips —
/// the env factory the base module queues; registering the server dedups with
/// any gateway's own `WsModule` import.
pub struct WsSetup {
    pinned: Option<WsConfig>,
}

impl DynamicModule for WsSetup {
    fn collect(&self, builder: ContainerBuilder) -> ContainerBuilder {
        ConfigModule::provide_feature(self.pinned.clone(), builder)
    }

    fn register(self, builder: ContainerBuilder) -> ContainerBuilder {
        <WsModule as Module>::register(builder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_core::{Container, Module};
    use std::sync::Arc;

    #[test]
    fn provides_the_server_registry() {
        let container = WsModule::register(Container::builder()).build();
        let server: Option<Arc<WsServer>> = container.get();
        assert!(server.is_some());
    }

    #[test]
    fn for_root_pins_the_config_and_still_provides_the_server() {
        use std::time::Duration;

        // `for_root(Some(cfg))` provides the config directly in `collect`, so it
        // is present without the AppBuilder factory phase.
        let setup =
            WsModule::for_root(WsConfig::default().with_max_connection(Duration::from_secs(42)));
        let builder = DynamicModule::collect(&setup, Container::builder());
        let container = setup.register(builder).build();

        let cfg: Option<Arc<WsConfig>> = container.get();
        assert_eq!(
            cfg.expect("pinned WsConfig resolves").max_connection,
            Some(Duration::from_secs(42)),
        );
        let server: Option<Arc<WsServer>> = container.get();
        assert!(server.is_some(), "for_root still provides the registry");
    }
}
