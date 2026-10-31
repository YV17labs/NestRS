//! [`WsModule`] — provides the [`WsServer`] connection registries, namespaced
//! ones included, and resolves [`WsConfig`].

use std::borrow::Cow;

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::__private::{EdgeHosts, EdgeRuntime, attaches_transport, registered_hosts};
use nest_rs_core::{Container, Edge, module};
use nest_rs_http::{HttpEndpointMeta, HttpTransport};

use crate::config::WsConfig;
use crate::namespace::WsNamespaces;
use crate::server::WsServer;

/// DI module that provides the [`WsServer`] connection registries and resolves
/// [`WsConfig`]. Import it wherever a gateway broadcasts or a service pushes to
/// clients.
///
/// It attaches no transport: a gateway upgrades from the HTTP transport its
/// adapter imports beside it (`HttpModule`), and a port pushing to clients
/// opens no listener in a binary serving no gateway. A gateway the app reaches
/// without it is named at `warn` when the app runs.
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

nest_rs_core::inventory::submit! {
    EdgeRuntime {
        edge: Edge::Ws,
        import: "nest_rs::http::HttpModule",
        attached: attaches_transport::<HttpTransport>,
        hosts: gateway_hosts,
    }
}

/// Every gateway the app's modules registered: each self-mounts its upgrade.
fn gateway_hosts(container: &Container) -> EdgeHosts {
    registered_hosts::<HttpEndpointMeta>(container, |meta| {
        (meta.label() == crate::gateway::GATEWAY_LABEL).then(|| Cow::Owned(meta.owner().to_owned()))
    })
}

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
