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

// ── Namespaced registries are owned by WsModule too ─────────────────────────
//
// A namespaced gateway used to install its own `WsServer<Ns>` from
// `Discoverable::register`. That made the key belong to no module, so the access
// graph's escape hatch for imperatively-registered types waved every consumer
// through: the app booted, mounted its routes, and *then* panicked at first
// resolution naming whichever provider happened to be built first. It was also
// order-sensitive — a consumer in a module the gateway *imports* is registered
// before the gateway exists.
//
// `WsModule` now owns every registry. Namespaced or not, the import is what the
// graph checks and the diagnostic is the same shape.

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

/// The finding: without the import, the boot used to succeed and panic later,
/// blaming an unrelated provider. It must fail at boot, naming the missing
/// dependency **with its marker** and the module that provides it.
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

/// The layout the CLI scaffolds and the campaign hit: the pushing service lives
/// in a module the gateway's module **imports**, so it is registered *before* the
/// gateway. Ordering used to decide whether this worked; now the service imports
/// the owner directly and the direction stops mattering.
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

// ── The unguarded-edge boot warning ─────────────────────────────────────────
//
// A gateway's `#[use_guards]` run inside the opaque mount closure, so the
// transport used to warn "unguarded self-mount edges detected" on *every*
// gateway whenever no global guard pool was registered — including one that
// rejects unauthenticated upgrades, with a hint recommending exactly what it
// already did. A security warning that fires on correct code is one people
// learn to scroll past, so the gateway now declares the fact.

use nest_rs_core::{Layer, Transport, injectable};
use nest_rs_guards::{Denial, Guard};
use nest_rs_testing::LogCapture;
use nest_rs_ws::async_trait;
use nest_rs_ws::nest_rs_http::HttpTransport;
use nest_rs_ws::nest_rs_http::poem::Request as HttpRequest;

#[injectable]
#[derive(Default)]
struct TicketGuard;

impl Layer for TicketGuard {}

#[async_trait]
impl Guard for TicketGuard {
    async fn check_http(&self, req: &mut HttpRequest) -> Result<(), Denial> {
        match req.headers().get("x-ticket") {
            Some(_) => Ok(()),
            None => Err(Denial::unauthorized("missing ticket")),
        }
    }
}

#[gateway(path = "/guarded")]
#[use_guards(TicketGuard)]
struct GuardedGateway;

#[messages]
impl GuardedGateway {
    #[subscribe_message("ping")]
    async fn ping(&self) -> String {
        "pong".into()
    }
}

#[module(imports = [WsModule], providers = [GuardedGateway, TicketGuard])]
struct GuardedModule;

const UNGUARDED_EDGES: &str = "unguarded self-mount edges detected";

async fn configure_and_capture<M: nest_rs_core::Module + 'static>() -> Vec<String> {
    let app = App::builder()
        .module::<M>()
        .build()
        .await
        .expect("the module builds");
    let logs = LogCapture::install();
    let mut transport = HttpTransport::new();
    transport
        .configure(app.container())
        .await
        .expect("a lone gateway mounts");
    logs.find("nest_rs::layers", UNGUARDED_EDGES)
        .into_iter()
        .map(|e| e.field("endpoints").unwrap_or_default())
        .collect()
}

#[tokio::test]
async fn a_gateway_binding_its_own_guards_is_not_reported_as_an_unguarded_edge() {
    let warned = configure_and_capture::<GuardedModule>().await;
    assert!(
        warned.is_empty(),
        "a gateway carrying #[use_guards] must not be listed as unguarded: {warned:?}",
    );
}

#[tokio::test]
async fn a_gateway_with_no_guard_at_all_is_still_reported() {
    // The other half: silencing the false positive must not silence the signal.
    let warned = configure_and_capture::<WiredModule>().await;
    assert!(
        warned.iter().any(|e| e.contains("/wired")),
        "an edge with neither global nor gateway guards must still warn: {warned:?}",
    );
}
