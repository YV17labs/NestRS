//! Wiring a gateway: a listed gateway brings the registry it reads and the HTTP
//! transport it upgrades from, and a port importing `WsModule` opens nothing.

use std::sync::Arc;

use nest_rs_core::{App, AppLifecycle, Module, injectable, module, target};
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_testing::{HeadlessApp, LogCapture};
use nest_rs_ws::{WsClient, WsConfig, WsModule, WsServer, gateway, messages};

#[gateway(path = "/wired")]
struct WiredGateway;

#[messages]
impl WiredGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> String {
        "pong".into()
    }
}

/// An adapter as a developer writes it: the gateway, and nothing of the edge's.
#[module(providers = [WiredGateway])]
struct WiredModule;

/// The app `M` composes, booted with no transport started.
async fn booted<M: Module>() -> HeadlessApp {
    HeadlessApp::builder()
        .module::<M>()
        .build_headless()
        .await
        .expect("the module boots")
}

#[tokio::test]
async fn a_listed_gateway_brings_the_registry_and_the_transport_it_upgrades_from() {
    let app = booted::<WiredModule>().await;

    assert!(app.container().get::<WsServer>().is_some());
    assert!(
        app.container().get::<WsConfig>().is_some(),
        "the bare import reads the edge's variables",
    );
    assert_eq!(app.attached_transports(), ["http"]);
}

/// The port a gateway's adapter imports: it pushes to clients, and a binary
/// reaching it without the adapter, a worker, must open no listener.
#[injectable]
struct Pusher {
    #[inject]
    server: Arc<WsServer>,
}

#[module(imports = [WsModule], providers = [Pusher])]
struct PushingPortModule;

#[tokio::test]
async fn a_port_importing_the_registry_opens_no_listener() {
    let app = booted::<PushingPortModule>().await;

    assert_eq!(
        app.container()
            .get::<Pusher>()
            .map(|pusher| pusher.server.connection_count()),
        Some(0),
    );
    assert!(app.attached_transports().is_empty());
}

/// The gateway's adapter beside the pinned transport, on a free loopback port.
#[module(imports = [HttpModule::for_root(loopback()), WiredModule])]
struct ServedWiredModule;

fn loopback() -> HttpConfig {
    HttpConfig {
        host: "127.0.0.1".into(),
        port: 0,
        ..HttpConfig::default()
    }
}

#[tokio::test]
async fn a_pinned_transport_serves_a_listed_gateway() {
    let logs = LogCapture::install();
    let app = App::builder()
        .module::<ServedWiredModule>()
        .build()
        .await
        .expect("the module boots");
    let lifecycle = app
        .container()
        .get::<AppLifecycle>()
        .expect("every app is seeded its lifecycle");

    let running = tokio::spawn(app.run());
    assert!(lifecycle.ready().await, "the app serves");
    running.abort();

    logs.expect_one(target::APP, "app ready");
}

struct Rooms;

#[gateway(path = "/rooms", namespace = Rooms)]
struct NamespacedGateway;

#[messages]
impl NamespacedGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self, client: &WsClient) -> String {
        let _ = client;
        "pong".into()
    }
}

#[module(providers = [NamespacedGateway])]
struct NamespacedModule;

#[tokio::test]
async fn a_listed_namespaced_gateway_brings_its_registry() {
    let app = App::builder()
        .module::<NamespacedModule>()
        .build()
        .await
        .expect("the gateway's namespaced registry comes with it");
    assert!(
        app.container().get::<WsServer<Rooms>>().is_some(),
        "the marker's registry is installed by WsModule's WsNamespaces provider",
    );
    assert!(
        app.container().get::<WsServer>().is_some(),
        "and the default one is still there — the two are separate singletons",
    );
}

/// The pushing service lives in a module the gateway's module imports, so it is
/// registered before the gateway.
#[injectable]
struct RoomsNotifier {
    #[inject]
    server: Arc<WsServer<Rooms>>,
}

impl RoomsNotifier {
    fn live(&self) -> usize {
        self.server.connection_count()
    }
}

#[module(imports = [WsModule], providers = [RoomsNotifier])]
struct RoomsFeatureModule;

#[module(imports = [RoomsFeatureModule], providers = [NamespacedGateway])]
struct RoomsAdapterModule;

#[tokio::test]
async fn a_service_in_a_module_the_gateway_imports_still_resolves_the_registry() {
    let app = App::builder()
        .module::<RoomsAdapterModule>()
        .build()
        .await
        .expect("the registry's owner is a leaf module, so import order is irrelevant");
    let notifier = app
        .container()
        .get::<RoomsNotifier>()
        .expect("RoomsNotifier is built");
    assert_eq!(notifier.live(), 0, "and it holds a real, empty registry");
}
