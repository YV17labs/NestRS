//! [`WsModule`] — provides the [`WsServer`] connection registries, namespaced
//! ones included, and resolves [`WsConfig`].

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::module;

use crate::config::WsConfig;
use crate::namespace::WsNamespaces;
use crate::server::WsServer;

/// DI module that provides the [`WsServer`] connection registries and resolves
/// [`WsConfig`]. Import it wherever a gateway broadcasts or a service pushes to
/// clients.
#[module(imports = [ConfigModule::for_feature::<WsConfig>()], providers = [WsServer, WsNamespaces])]
pub struct WsModule;

impl WsModule {
    /// `None` ⇒ load [`WsConfig`] from `<PREFIX>_WS__*` over its defaults;
    /// `Some(cfg)` makes `cfg` the base those variables overlay.
    pub fn for_root(config: impl Into<Option<WsConfig>>) -> WsSetup {
        ConfigModule::setup(config)
    }
}

/// [`DynamicModule`](nest_rs_core::DynamicModule) returned by
/// [`WsModule::for_root`]: resolves [`WsConfig`] (env over the pinned base),
/// then brings the base [`WsModule`] wiring.
pub type WsSetup = ConfigSetup<WsModule, WsConfig>;

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_core::Container;
    use std::sync::Arc;

    #[test]
    fn provides_the_server_registry() {
        let container = Container::builder().import::<WsModule>().build();
        let server: Option<Arc<WsServer>> = container.get();
        assert!(server.is_some());
    }

    fn pinned_ws() -> WsSetup {
        WsModule::for_root(
            WsConfig::default().with_max_connection(std::time::Duration::from_secs(42)),
        )
    }

    #[module(imports = [pinned_ws()])]
    struct PinnedWsHost;

    #[tokio::test]
    async fn for_root_pins_the_config_and_still_provides_the_server() {
        use nest_rs_core::App;
        use std::time::Duration;

        // The pinned value materializes in the factory phase, so the app is booted.
        let app = App::builder()
            .module::<PinnedWsHost>()
            .build()
            .await
            .expect("the pinned-config module boots");

        let cfg: Option<Arc<WsConfig>> = app.container().get();
        assert_eq!(
            cfg.expect("pinned WsConfig resolves").max_connection,
            Some(Duration::from_secs(42)),
        );
        let server: Option<Arc<WsServer>> = app.container().get();
        assert!(server.is_some(), "for_root still provides the registry");
    }
}
