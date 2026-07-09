//! Layer-pool invariant: a layer declared at any combination of scopes
//! (global / controller / method) executes **exactly once** per request. This
//! is the contract the unified layer pool guarantees — scope is a *declaration*
//! concern, deduplicated by `TypeId` via `compose_chain`, run once.
//!
//! The first half pins the invariant for **guards** across all seven scope
//! combinations (interceptor / filter / pipe / exception-filter cross-scope
//! dedup is exercised in `interceptors.rs`, `versioning_filters.rs`,
//! `pipes.rs` and `exception_filters.rs`). A counting guard increments a
//! process-global counter and then lets the request through, so the response
//! is always `200` and the counter is the assertion surface.
//!
//! The second half pins the **execution site** contract for the global
//! interceptor / filter scope: they run at the transport edge, so they
//! observe what the per-route site cannot — 404s and guard denials — and an
//! error a route-site filter maps carries the `MappedError` marker the data
//! layer reads to refuse the commit.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, MappedError, injectable, module};
use nest_rs_filters::{Filter, RequestSnapshot, filter};
use nest_rs_guards::{Denial, Guard, guard};
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_interceptors::{Interceptor, Next, interceptor};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;
use poem::{Request, Response};
use tokio::sync::Mutex;

static COUNTER: AtomicUsize = AtomicUsize::new(0);
static GATE: Mutex<()> = Mutex::const_new(());

fn reset_counter() {
    COUNTER.store(0, Ordering::SeqCst);
}

fn counter() -> usize {
    COUNTER.load(Ordering::SeqCst)
}

/// Counts every execution, then admits the request (so the count, not the
/// status, is what each test asserts).
#[injectable]
#[derive(Default)]
struct CountingGuard;

impl Layer for CountingGuard {}

#[async_trait]
impl Guard for CountingGuard {
    async fn check_http(&self, _req: &mut Request) -> std::result::Result<(), Denial> {
        COUNTER.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// One controller carries every scope combination on its own route, so a single
// module covers all cases. `g` = guarded at the named scope(s).

#[controller(path = "/pool")]
#[use_guards(CountingGuard)]
struct ControllerGuarded;

#[routes]
impl ControllerGuarded {
    // controller-only
    #[get("/c")]
    async fn c(&self) -> &'static str {
        "ok"
    }

    // controller + method
    #[get("/cm")]
    #[use_guards(CountingGuard)]
    async fn cm(&self) -> &'static str {
        "ok"
    }
}

#[controller(path = "/pool-m")]
struct MethodGuarded;

#[routes]
impl MethodGuarded {
    // method-only
    #[get("/m")]
    #[use_guards(CountingGuard)]
    async fn m(&self) -> &'static str {
        "ok"
    }

    // unguarded locally — used only under a global guard
    #[get("/none")]
    async fn none(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [CountingGuard, ControllerGuarded, MethodGuarded])]
struct PoolModule;

/// controller, method, and controller+method — no global seeding.
#[tokio::test]
async fn guard_runs_once_per_request_at_each_local_scope() {
    let _gate = GATE.lock().await;
    let app = TestApp::for_module::<PoolModule>().await.expect("boots");

    for path in ["/pool/c", "/pool/cm", "/pool-m/m"] {
        reset_counter();
        app.http().get(path).send().await.assert_status_is_ok();
        assert_eq!(counter(), 1, "guard at {path} must execute exactly once");
    }
}

/// global alone, global+controller, global+method, global+controller+method.
#[tokio::test]
async fn guard_runs_once_per_request_with_global_seeding() {
    let _gate = GATE.lock().await;
    let app = TestApp::builder()
        .module::<PoolModule>()
        .use_guards_global([guard::<CountingGuard>()])
        .build()
        .await
        .expect("boots with a global guard");

    // /pool-m/none: global only. /pool/c: global+controller.
    // /pool-m/m: global+method. /pool/cm: global+controller+method.
    for path in ["/pool-m/none", "/pool/c", "/pool-m/m", "/pool/cm"] {
        reset_counter();
        app.http().get(path).send().await.assert_status_is_ok();
        assert_eq!(
            counter(),
            1,
            "guard declared global + local at {path} must still execute exactly once",
        );
    }
}

// --- transport-edge coverage of the global interceptor / filter scope --------
//
// Global interceptors / filters execute at the transport edge (outside
// routing and the guard pool), so they cover what a per-route site cannot:
// unmatched paths and guard denials. Per-route interceptors stay inside the
// guard chain (a denial short-circuits before them) — that case is pinned in
// `interceptors.rs::guard_short_circuits_before_the_interceptor`.

static EDGE_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn reset_edge_counter() {
    EDGE_COUNTER.store(0, Ordering::SeqCst);
}

fn edge_counter() -> usize {
    EDGE_COUNTER.load(Ordering::SeqCst)
}

/// Counts every request it observes, then forwards.
#[injectable]
#[derive(Default)]
struct EdgeObserver;

impl Layer for EdgeObserver {}

#[async_trait]
impl Interceptor for EdgeObserver {
    async fn intercept(&self, req: Request, next: Next<'_>) -> poem::Result<Response> {
        EDGE_COUNTER.fetch_add(1, Ordering::SeqCst);
        next.run(req).await
    }
}

/// Maps any error escaping routing to `418` with a fixed body.
#[injectable]
#[derive(Default)]
struct EdgeTeapot;

impl Layer for EdgeTeapot {}

#[async_trait]
impl Filter for EdgeTeapot {
    async fn filter(&self, _req: &RequestSnapshot, _error: poem::Error) -> Response {
        Response::builder()
            .status(StatusCode::IM_A_TEAPOT)
            .body("edge-mapped")
    }
}

/// Maps a handler error to a *success* — exists to prove the response still
/// carries the `MappedError` marker the data layer reads to refuse a commit.
#[injectable]
#[derive(Default)]
struct MapToOk;

impl Layer for MapToOk {}

#[async_trait]
impl Filter for MapToOk {
    async fn filter(&self, _req: &RequestSnapshot, _error: poem::Error) -> Response {
        Response::builder().status(StatusCode::OK).body("recovered")
    }
}

#[injectable]
#[derive(Default)]
struct DenyAll;

impl Layer for DenyAll {}

#[async_trait]
impl Guard for DenyAll {
    async fn check_http(&self, _req: &mut Request) -> std::result::Result<(), Denial> {
        Err(Denial::forbidden("denied"))
    }
}

#[controller(path = "/edge")]
struct EdgeController;

#[routes]
impl EdgeController {
    #[get("/denied")]
    #[use_guards(DenyAll)]
    async fn denied(&self) -> &'static str {
        "unreachable"
    }

    #[get("/boom")]
    #[use_filters(MapToOk)]
    async fn boom(&self) -> poem::Result<&'static str> {
        Err(poem::Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

#[module(providers = [EdgeObserver, EdgeTeapot, MapToOk, DenyAll, EdgeController])]
struct EdgeModule;

#[tokio::test]
async fn global_interceptor_observes_an_unmatched_path() {
    let _gate = GATE.lock().await;
    reset_edge_counter();

    let app = TestApp::builder()
        .module::<EdgeModule>()
        .use_interceptors_global([interceptor::<EdgeObserver>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/no-such-route").send().await;
    resp.assert_status(StatusCode::NOT_FOUND);
    assert_eq!(
        edge_counter(),
        1,
        "a global interceptor wraps routing itself, so a 404 is still observed",
    );
}

#[tokio::test]
async fn global_interceptor_observes_a_guard_denial() {
    let _gate = GATE.lock().await;
    reset_edge_counter();

    let app = TestApp::builder()
        .module::<EdgeModule>()
        .use_interceptors_global([interceptor::<EdgeObserver>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/edge/denied").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);
    assert_eq!(
        edge_counter(),
        1,
        "a global interceptor runs outside the guard pool and observes the denial response",
    );
}

#[tokio::test]
async fn global_filter_maps_an_unmatched_path() {
    let _gate = GATE.lock().await;

    let app = TestApp::builder()
        .module::<EdgeModule>()
        .use_filters_global([filter::<EdgeTeapot>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/no-such-route").send().await;
    resp.assert_status(StatusCode::IM_A_TEAPOT);
    resp.assert_text("edge-mapped").await;
}

#[tokio::test]
async fn a_mapped_error_response_carries_the_rollback_marker() {
    let _gate = GATE.lock().await;

    let app = TestApp::for_module::<EdgeModule>().await.expect("boots");

    // The route-site filter maps the handler's 500 into a 200 — but the
    // response must carry `MappedError`, which the data layer reads to roll
    // the ambient transaction back (a mapped error never commits).
    let resp = app.http().get("/edge/boom").send().await;
    resp.assert_status_is_ok();
    assert!(
        resp.0.extensions().get::<MappedError>().is_some(),
        "a response produced by mapping a handler error must be tagged MappedError",
    );
}
