//! Per-handler / per-controller interceptor binding + guard-before-interceptor
//! ordering, end-to-end through the HTTP harness. Also pins the cross-scope
//! TypeId dedup: an interceptor declared globally is run by the transport-
//! level [`HttpInterceptorMeta`] wrap; a redeclaration at controller or
//! method scope is skipped at mount time, so the interceptor still executes
//! exactly once.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_interceptors::{Interceptor, Next, interceptor};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;
use poem::{Request, Response, Result};
use tokio::sync::Mutex;

#[injectable]
#[derive(Default)]
struct Tracer;

impl Layer for Tracer {}

#[async_trait]
impl Interceptor for Tracer {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        let mut resp = next.run(req).await?;
        resp.headers_mut()
            .insert("x-trace", "hit".parse().expect("static header value"));
        Ok(resp)
    }
}

#[injectable]
#[derive(Default)]
struct DenyGuard;

impl Layer for DenyGuard {}

#[async_trait]
impl Guard for DenyGuard {
    async fn check_http(&self, _req: &mut Request) -> std::result::Result<(), Denial> {
        Err(Denial::forbidden("denied"))
    }
}

#[controller(path = "/a")]
struct PerHandlerController;

#[routes]
impl PerHandlerController {
    #[get("/traced")]
    #[use_interceptors(Tracer)]
    async fn traced(&self) -> &'static str {
        "ok"
    }

    #[get("/plain")]
    async fn plain(&self) -> &'static str {
        "ok"
    }

    #[get("/denied")]
    #[use_guards(DenyGuard)]
    #[use_interceptors(Tracer)]
    async fn denied(&self) -> &'static str {
        "unreachable"
    }
}

#[controller(path = "/b")]
#[use_interceptors(Tracer)]
struct PerControllerController;

#[routes]
impl PerControllerController {
    #[get("/one")]
    async fn one(&self) -> &'static str {
        "ok"
    }

    #[get("/two")]
    async fn two(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [Tracer, DenyGuard, PerHandlerController, PerControllerController])]
struct InterceptorModule;

#[tokio::test]
async fn per_handler_interceptor_stamps_only_its_route() {
    let app = TestApp::for_module::<InterceptorModule>()
        .await
        .expect("boots");

    let traced = app.http().get("/a/traced").send().await;
    traced.assert_status_is_ok();
    traced.assert_header("x-trace", "hit");

    let plain = app.http().get("/a/plain").send().await;
    plain.assert_status_is_ok();
    plain.assert_header_is_not_exist("x-trace");
}

#[tokio::test]
async fn per_controller_interceptor_stamps_every_route() {
    let app = TestApp::for_module::<InterceptorModule>()
        .await
        .expect("boots");

    for path in ["/b/one", "/b/two"] {
        let resp = app.http().get(path).send().await;
        resp.assert_status_is_ok();
        resp.assert_header("x-trace", "hit");
    }
}

#[tokio::test]
async fn guard_short_circuits_before_the_interceptor() {
    let app = TestApp::for_module::<InterceptorModule>()
        .await
        .expect("boots");

    let resp = app.http().get("/a/denied").send().await;
    resp.assert_status(StatusCode::FORBIDDEN);
    resp.assert_header_is_not_exist("x-trace");
}

// --- TypeId dedup across scopes ---------------------------------------------
//
// The interceptor under test ([`CounterInterceptor`]) bumps a process-global
// counter every time `intercept` runs, then forwards. Tests share that
// counter, so a `tokio::sync::Mutex` serializes them — `cargo nextest`
// parallelizes by default.

static COUNTER: AtomicUsize = AtomicUsize::new(0);
static GATE: Mutex<()> = Mutex::const_new(());

fn reset_counter() {
    COUNTER.store(0, Ordering::SeqCst);
}

fn counter() -> usize {
    COUNTER.load(Ordering::SeqCst)
}

#[injectable]
#[derive(Default)]
struct CounterInterceptor;

impl Layer for CounterInterceptor {}

#[async_trait]
impl Interceptor for CounterInterceptor {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        COUNTER.fetch_add(1, Ordering::SeqCst);
        next.run(req).await
    }
}

#[controller(path = "/dup-global-ctrl")]
#[use_interceptors(CounterInterceptor)]
struct DupGlobalCtrl;

#[routes]
impl DupGlobalCtrl {
    #[get("/echo")]
    async fn echo_dup_global_ctrl(&self) -> &'static str {
        "ok"
    }
}

#[controller(path = "/dup-global-method")]
struct DupGlobalMethod;

#[routes]
impl DupGlobalMethod {
    #[get("/echo")]
    #[use_interceptors(CounterInterceptor)]
    async fn echo_dup_global_method(&self) -> &'static str {
        "ok"
    }
}

#[controller(path = "/dup-ctrl-method")]
#[use_interceptors(CounterInterceptor)]
struct DupCtrlMethod;

#[routes]
impl DupCtrlMethod {
    #[get("/echo")]
    #[use_interceptors(CounterInterceptor)]
    async fn echo_dup_ctrl_method(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [
    CounterInterceptor,
    DupGlobalCtrl,
    DupGlobalMethod,
    DupCtrlMethod,
])]
struct DedupModule;

#[tokio::test]
async fn same_interceptor_global_and_controller_runs_once() {
    let _gate = GATE.lock().await;
    reset_counter();

    // Declared globally AND redeclared on the controller. The controller-
    // scope wrap is skipped at mount time when the TypeId is already
    // seeded as Global — the transport-level `HttpInterceptorMeta` wrap
    // from `use_interceptors_global` carries the single execution.
    let app = TestApp::builder()
        .module::<DedupModule>()
        .use_interceptors_global([interceptor::<CounterInterceptor>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/dup-global-ctrl/echo").send().await;
    resp.assert_status_is_ok();
    assert_eq!(
        counter(),
        1,
        "TypeId dedup: global + controller redeclaration must execute once",
    );
}

#[tokio::test]
async fn same_interceptor_global_and_method_runs_once() {
    let _gate = GATE.lock().await;
    reset_counter();

    // Same shape as the controller case — the method-scope wrap is
    // skipped at mount time because the TypeId is already seeded as
    // Global.
    let app = TestApp::builder()
        .module::<DedupModule>()
        .use_interceptors_global([interceptor::<CounterInterceptor>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/dup-global-method/echo").send().await;
    resp.assert_status_is_ok();
    assert_eq!(
        counter(),
        1,
        "TypeId dedup: global + method redeclaration must execute once",
    );
}

#[tokio::test]
async fn same_interceptor_controller_and_method_without_global_executes_twice() {
    let _gate = GATE.lock().await;
    reset_counter();

    // With no Global seeding, the controller and method wraps each
    // execute the interceptor — the HTTP per-scope dedup is wired
    // against `InterceptorSpecs` (Global) only, mirroring the v5
    // contract for `Interceptor` / `Filter` (the unified pipe /
    // exception-filter chain dedups across all three scopes via
    // `compose_chain`, but those two go through dedicated per-scope
    // wraps and only skip when a Global declaration matches).
    let app = TestApp::for_module::<DedupModule>().await.expect("boots");

    let resp = app.http().get("/dup-ctrl-method/echo").send().await;
    resp.assert_status_is_ok();
    assert_eq!(
        counter(),
        2,
        "controller + method without Global currently run both wraps",
    );
}
