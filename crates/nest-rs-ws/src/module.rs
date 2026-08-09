//! [`WsModule`] — provides the [`WsServer`] connection registry and resolves
//! [`WsConfig`] (the socket-lifetime ceiling). Import it in any module whose
//! gateways broadcast or whose services push to clients, so `#[inject]
//! Arc<WsServer>` resolves and passes the access graph.
//!
//! It owns the **namespaced** registries too: a `#[gateway(namespace = N)]`
//! submits its marker and [`WsNamespaces`] installs the matching
//! `WsServer<N>`, so `Arc<WsServer<NotifyNs>>` is reachable on exactly the same
//! terms as the default one — one import, one named boot error when it is
//! missing, no dependence on where the gateway sits in the module tree. See
//! `crate::namespace` for what that replaced.
//!
//! [`WsConfig`] loads from `NESTRS_WS__*` by default (importing `WsModule` is
//! enough); [`WsModule::for_root`] supplies a base for those variables to
//! overlay, so a field pinned in code is still overridable per field by the
//! deployment (see `nest_rs_config::Config`).

use nest_rs_config::{ConfigModule, ConfigSetup};
use nest_rs_core::module;

use crate::config::WsConfig;
use crate::namespace::WsNamespaces;
use crate::server::WsServer;

/// DI module that provides the [`WsServer`] connection registry and resolves
/// [`WsConfig`]. Import it wherever a gateway broadcasts or a service pushes to
/// clients; see the module docs.
#[module(imports = [ConfigModule::for_feature::<WsConfig>()], providers = [WsServer, WsNamespaces])]
pub struct WsModule;

impl WsModule {
    /// `None` ⇒ load [`WsConfig`] from `NESTRS_WS__*` over its defaults;
    /// `Some(cfg)` makes `cfg` the base those variables overlay. Either way the
    /// [`WsServer`] registry is provided, so this is a drop-in replacement for
    /// importing the bare [`WsModule`].
    pub fn for_root(config: impl Into<Option<WsConfig>>) -> WsSetup {
        ConfigModule::setup(config)
    }
}

/// [`DynamicModule`] returned by [`WsModule::for_root`]: resolves [`WsConfig`]
/// (env over the pinned base), then brings the base [`WsModule`] wiring (the
/// [`WsServer`] registry). This factory is queued first, so it wins over — and
/// skips — the plain env factory the base module queues; registering the server
/// dedups with any gateway's own `WsModule` import.
///
/// Pin-and-recurse is the whole behaviour, so it is [`ConfigSetup`] rather than
/// a type of its own.
pub type WsSetup = ConfigSetup<WsModule, WsConfig>;

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

    /// Pinned ceiling for the `for_root` test below, as a real import site.
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

        // `for_root(Some(cfg))` queues the resolving factory rather than
        // providing the struct verbatim — that is what keeps `NESTRS_WS__*` live
        // for every field the call site did not pin — so the value materializes
        // in the AppBuilder's factory phase. Boot the app rather than driving
        // `collect` by hand.
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
