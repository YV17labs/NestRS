//! Wiring a gateway: what happens when `WsModule` is missing.
//!
//! The connection registry is not optional for a default-namespace gateway —
//! every `WsClient` a handler touches reads it. It used to be resolved with an
//! `.expect(...)` at mount, so an app compiled, logged
//! `mounted endpoint kind="ws"`, and *then* panicked with a `RUST_BACKTRACE`
//! note. Every other boot-time misconfiguration in the framework exits cleanly
//! through the access graph, naming both the missing type and the module that
//! provides it; this one now does too.

use std::sync::Arc;

use nest_rs_core::{AccessGraphError, App, module};
use nest_rs_ws::{WsClient, WsModule, WsServer, gateway, messages};

#[gateway(path = "/unwired")]
struct UnwiredGateway;

#[messages]
impl UnwiredGateway {
    #[subscribe_message("ping")]
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
    async fn ping(&self) -> String {
        "pong".into()
    }
}

#[module(imports = [WsModule], providers = [WiredGateway])]
struct WiredModule;

#[tokio::test]
async fn importing_ws_module_is_the_whole_fix() {
    let app = App::builder()
        .module::<WiredModule>()
        .build()
        .await
        .expect("the import the error names is sufficient");
    assert!(app.container().get::<WsServer>().is_some());
}

/// A namespaced gateway self-provides its own `WsServer<Ns>`, so the same
/// declaration is satisfied without importing anything — the registry
/// dependency must not turn `namespace` into a second wiring step.
struct Rooms;

#[gateway(path = "/rooms", namespace = Rooms)]
struct NamespacedGateway;

#[messages]
impl NamespacedGateway {
    #[subscribe_message("ping")]
    async fn ping(&self, client: &WsClient) -> String {
        let _ = client;
        "pong".into()
    }
}

#[module(providers = [NamespacedGateway])]
struct NamespacedModule;

#[tokio::test]
async fn a_namespaced_gateway_needs_no_import() {
    let app = App::builder()
        .module::<NamespacedModule>()
        .build()
        .await
        .expect("a namespaced gateway provides its own registry");
    assert!(app.container().get::<Arc<WsServer<Rooms>>>().is_none());
    assert!(app.container().get::<WsServer<Rooms>>().is_some());
}
