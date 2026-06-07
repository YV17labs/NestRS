//! WebSocket gateway-scope guard dedup against Global on the WS upgrade.
//!
//! A WS upgrade is an HTTP `GET`; the global guard chain runs through the
//! transport-level [`GlobalGuardsHttpInterceptor`]. A gateway that
//! redeclares the same guard via `#[use_guards(...)]` would otherwise wrap
//! the upgrade endpoint twice — `#[gateway]` skips its inline wrap at
//! mount time when the TypeId matches a `GuardSpecs` entry. The check
//! still runs exactly once and a denial still short-circuits the upgrade
//! before any `WebSocket::from_request` work.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, guard};
use nest_rs_http::async_trait;
use nest_rs_testing::TestApp;
use nest_rs_ws::{WsModule, gateway, messages};
use poem::Request;
use poem::http::StatusCode;
use tokio::sync::Mutex;

// --- shared observable state -------------------------------------------------

static COUNTER: AtomicUsize = AtomicUsize::new(0);
static GATE: Mutex<()> = Mutex::const_new(());

fn reset_counter() {
    COUNTER.store(0, Ordering::SeqCst);
}

fn counter() -> usize {
    COUNTER.load(Ordering::SeqCst)
}

// --- a counting deny guard ---------------------------------------------------

/// Increments [`COUNTER`] every time it runs, then denies with `403`. The
/// counter lets the dedup test distinguish "ran once" from "ran twice but
/// only the first response is observed".
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

// --- two gateways: one bare (relies on Global), one redeclaring the guard ---

#[gateway(path = "/ws-bare")]
struct BareGateway;

#[messages]
impl BareGateway {
    #[subscribe_message("ping")]
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
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

#[module(imports = [WsModule], providers = [CountingDenyGuard, BareGateway, DupGateway])]
struct GatewayDedupModule;

// --- the test ----------------------------------------------------------------

#[tokio::test]
async fn gateway_scope_guard_redeclared_against_global_runs_once() {
    let _gate = GATE.lock().await;
    reset_counter();

    // Global declares `CountingDenyGuard`; the `DupGateway` redeclares it
    // on the gateway struct. `#[gateway]` skips its inline wrap because
    // the TypeId is in `GuardSpecs`, so only the transport-level
    // `GlobalGuardsHttpInterceptor` runs the guard — counter bumps once.
    let app = TestApp::builder()
        .module::<GatewayDedupModule>()
        .use_guards_global([guard::<CountingDenyGuard>()])
        .build()
        .await
        .expect("boots");

    // Plain GET (no WS upgrade headers) — the guard fires before
    // `WebSocket::from_request` would reject the missing upgrade, so we
    // observe the 403 and the counter increment without needing a real
    // socket.
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

    // Sanity check: a gateway with no `#[use_guards]` still has the
    // global guard applied through `GlobalGuardsHttpInterceptor`, exactly
    // once.
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

