//! A gateway redeclaring a global guard runs it once on the WS upgrade, and a
//! denial short-circuits the upgrade.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard, guard};
use nest_rs_http::async_trait;
use nest_rs_testing::TestApp;
use nest_rs_ws::{WsModule, gateway, messages};
use poem::Request;
use poem::http::StatusCode;
use tokio::sync::Mutex;

static COUNTER: AtomicUsize = AtomicUsize::new(0);
static GATE: Mutex<()> = Mutex::const_new(());

fn reset_counter() {
    COUNTER.store(0, Ordering::SeqCst);
}

fn counter() -> usize {
    COUNTER.load(Ordering::SeqCst)
}

/// Increments [`COUNTER`] every time it runs, then denies with `403`.
#[injectable]
#[derive(Default)]
struct CountingDenyGuard;

impl Layer for CountingDenyGuard {}

#[async_trait]
impl Guard for CountingDenyGuard {
    async fn check_http(&self, _req: &mut Request) -> std::result::Result<(), Denial> {
        COUNTER.fetch_add(1, Ordering::SeqCst);
        Err(Denial::forbidden("counted-and-denied"))
    }
}

impl HttpGuard for CountingDenyGuard {}

#[gateway(path = "/ws-bare")]
struct BareGateway;

#[messages]
impl BareGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

#[gateway(path = "/ws-dup")]
#[use_guards(CountingDenyGuard)]
struct DupGateway;

#[messages]
impl DupGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

#[module(imports = [WsModule], providers = [CountingDenyGuard, BareGateway, DupGateway])]
struct GatewayDedupModule;

#[tokio::test]
async fn gateway_scope_guard_redeclared_against_global_runs_once() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::builder()
        .module::<GatewayDedupModule>()
        .use_guards_global([guard::<CountingDenyGuard>()])
        .build()
        .await
        .expect("boots");

    // No upgrade headers: the guard fires before `WebSocket::from_request`.
    let resp = app.http().get("/ws-dup").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);

    assert_eq!(
        counter(),
        1,
        "gateway-scope redeclaration of a global guard must not double-fire on the WS upgrade",
    );
}

#[tokio::test]
async fn bare_gateway_runs_global_guard_once() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::builder()
        .module::<GatewayDedupModule>()
        .use_guards_global([guard::<CountingDenyGuard>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/ws-bare").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);

    assert_eq!(counter(), 1, "the global guard runs once on the WS upgrade");
}
