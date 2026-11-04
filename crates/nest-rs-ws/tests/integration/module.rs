//! Wiring a gateway: what happens when `WsModule` is missing, and when the
//! HTTP transport it upgrades from is or is not attached.

use std::sync::Arc;

use nest_rs_core::{AccessGraphError, App, AppLifecycle, injectable, module, target};
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_testing::LogCapture;
use nest_rs_ws::{WsClient, WsModule, WsServer, gateway, messages};

#[gateway(path = "/unwired")]
struct UnwiredGateway;

#[messages]
impl UnwiredGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> String {
        "pong".into()
    }
}

#[module(providers = [UnwiredGateway])]
struct UnwiredModule;

#[tokio::test]
async fn a_gateway_without_ws_module_fails_the_boot_naming_it() {
    let Err(err) = App::builder().module::<UnwiredModule>().build().await else {
        panic!("a default-namespace gateway cannot serve without the registry");
    };

    let violation = err
        .downcast_ref::<AccessGraphError>()
        .unwrap_or_else(|| panic!("expected an access-graph boot error, got: {err:#}"));
    assert_eq!(violation.consumer, "UnwiredGateway");
    assert_eq!(violation.dependency, "WsServer");
    assert_eq!(
        violation.owner, "WsModule",
        "the message names the module to import: {err:#}",
    );
    assert!(
        !format!("{err:#}").contains("RUST_BACKTRACE"),
        "a missing import is a wiring error, not a panic",
    );
}

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

#[module(imports = [WsModule], providers = [WiredGateway])]
struct WiredModule;

#[tokio::test]
async fn a_gateway_the_app_reaches_without_the_http_transport_is_named_at_run() {
    let logs = LogCapture::install();

    App::builder()
        .module::<WiredModule>()
        .build()
        .await
        .expect("the module boots")
        .run()
        .await
        .expect("an app serving nothing stops cleanly");

    let unserved = logs.expect_one(
        target::APP,
        "host not served: its edge's transport is not attached",
    );
    assert_eq!(unserved.level, "warn");
    assert_eq!(unserved.field("edge").as_deref(), Some("ws"));
    assert_eq!(unserved.field("host").as_deref(), Some("WiredGateway"));
    assert_eq!(unserved.field("module").as_deref(), Some("WiredModule"));
    assert_eq!(
        unserved.field("import").as_deref(),
        Some("nest_rs::http::HttpModule")
    );
}

/// The gateway's adapter beside the transport, on a free loopback port.
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
async fn a_gateway_the_transport_serves_is_not_named_at_run() {
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

    // `App::run` names an unserved host before it turns ready, so the run is
    // dropped there.
    let running = tokio::spawn(app.run());
    assert!(lifecycle.ready().await, "the app serves");
    running.abort();

    logs.expect_one(target::APP, "app ready");
    logs.expect_none(
        target::APP,
        "host not served: its edge's transport is not attached",
    );
}

#[tokio::test]
async fn importing_ws_module_is_the_whole_fix() {
    let app = App::builder()
        .module::<WiredModule>()
        .build()
        .await
        .expect("the import the error names is sufficient");
    assert!(app.container().get::<WsServer>().is_some());
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

#[module(imports = [WsModule], providers = [NamespacedGateway])]
struct WiredNamespacedModule;

#[tokio::test]
async fn a_namespaced_registry_comes_from_ws_module() {
    let app = App::builder()
        .module::<WiredNamespacedModule>()
        .build()
        .await
        .expect("importing WsModule provides the namespaced registry too");
    assert!(
        app.container().get::<WsServer<Rooms>>().is_some(),
        "the marker's registry is installed by WsModule's WsNamespaces provider",
    );
    assert!(
        app.container().get::<WsServer>().is_some(),
        "and the default one is still there — the two are separate singletons",
    );
}

#[tokio::test]
async fn a_namespaced_gateway_without_ws_module_fails_boot_by_name() {
    let Err(err) = App::builder().module::<NamespacedModule>().build().await else {
        panic!("the registry is a declared dependency like any other");
    };

    let violation = err
        .downcast_ref::<AccessGraphError>()
        .unwrap_or_else(|| panic!("expected an access-graph boot error, got: {err:#}"));
    assert_eq!(violation.consumer, "NamespacedGateway");
    assert_eq!(
        violation.dependency, "WsServer<Rooms>",
        "the key is named with its marker, so several namespaces stay distinguishable",
    );
    assert_eq!(
        violation.owner, "WsModule",
        "and the message names the module to import: {err:#}",
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
