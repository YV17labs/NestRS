//! [`WsModule`] — provides the [`WsServer`] connection registries, namespaced
//! ones included, and resolves [`WsConfig`].

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::__private::RuntimeModule;
use nest_rs_core::module;
use nest_rs_http::HttpModule;

use crate::config::WsConfig;
use crate::namespace::WsNamespaces;
use crate::server::WsServer;

/// DI module that provides the [`WsServer`] connection registries and resolves
/// [`WsConfig`]. A listed gateway brings it; import it where a service pushes
/// to clients.
///
/// It attaches no transport: a gateway brings the HTTP transport it upgrades
/// from itself, and a port pushing to clients opens no listener in a binary
/// serving no gateway.
#[module(imports = [nest_rs_config::__private::feature::<WsConfig>()], providers = [WsServer, WsNamespaces])]
pub struct WsModule;

impl WsModule {
    /// Pin `config` as the base `<PREFIX>_WS__*` overlays, per field; the
    /// bare import reads those variables over [`WsConfig`]'s defaults.
    pub fn for_root(config: WsConfig) -> WsSetup {
        ConfigModule::setup(config)
    }
}

/// [`DynamicModule`](nest_rs_core::DynamicModule) returned by
/// [`WsModule::for_root`]: resolves [`WsConfig`] (env over the pinned base),
/// then brings the base [`WsModule`] wiring.
pub type WsSetup = ConfigSetup<WsModule, WsConfig>;

/// What a gateway runs on, which `#[messages]` brings: the transport it
/// upgrades from, and the registry it reads.
pub const RUNTIME: &[RuntimeModule] = &[
    RuntimeModule::of::<HttpModule>(),
    RuntimeModule::of::<WsModule>(),
];

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_core::Container;
    use std::sync::Arc;

    #[test]
    fn provides_the_server_registry() {
        let container = Container::builder()
            .provide(WsConfig::default())
            .import::<WsModule>()
            .build()
            .expect("the container builds");
        let server: Option<Arc<WsServer>> = container.get();
        assert!(server.is_some());
    }

    fn pinned_ws() -> WsSetup {
        WsModule::for_root(WsConfig {
            max_connection: Some(std::time::Duration::from_secs(42)),
            ..WsConfig::default()
        })
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
