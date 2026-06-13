//! Per-handler / per-controller interceptor binding + guard-before-interceptor
//! ordering, end-to-end through the HTTP harness. Also pins the cross-scope
//! TypeId dedup: an interceptor declared at any combination of global /
//! controller / method scopes is composed through `compose_chain` by the
//! per-route pool and executes exactly once.

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

    // Declared globally AND redeclared on the controller. The per-route pool
    // composes global + controller and dedups by TypeId — broadest (global)
    // wins, so the interceptor runs once.
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

    // Same shape as the controller case — global + method compose and dedup
    // to a single execution (broadest scope wins).
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
async fn same_interceptor_controller_and_method_runs_once() {
    let _gate = GATE.lock().await;
    reset_counter();

    // Controller and method both declare `CounterInterceptor`. The per-route
    // pool composer (`wrap_route_interceptors`) runs every layer kind through
    // the same `compose_chain` dedup as guards / pipes — broadest scope wins —
    // so the interceptor executes exactly once, no Global declaration needed.
    let app = TestApp::for_module::<DedupModule>().await.expect("boots");

    let resp = app.http().get("/dup-ctrl-method/echo").send().await;
    resp.assert_status_is_ok();
    assert_eq!(
        counter(),
        1,
        "controller + method declaration dedups to a single execution",
    );
}

#[tokio::test]
async fn same_interceptor_at_all_three_scopes_runs_once() {
    let _gate = GATE.lock().await;
    reset_counter();

    // Global + controller + method — the broadest (global) wins and executes
    // at the transport edge; both narrower redeclarations are dropped.
    let app = TestApp::builder()
        .module::<DedupModule>()
        .use_interceptors_global([interceptor::<CounterInterceptor>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/dup-ctrl-method/echo").send().await;
    resp.assert_status_is_ok();
    assert_eq!(
        counter(),
        1,
        "global + controller + method declaration still executes exactly once",
    );
}

// --- infra `#[interceptor]` — transport-edge band, auto-mounted, non-provider ---

static EDGE_EVENTS: Mutex<Vec<&'static str>> = Mutex::const_new(Vec::new());

/// Infra interceptor (the `DbContext` shape): attached by `#[interceptor]`
/// as an `HttpEndpointWrap` at the transport edge — never a provider, never
/// in the per-route pool.
#[nest_rs_http::interceptor]
struct EdgeStamp;

impl Layer for EdgeStamp {}

#[async_trait]
impl Interceptor for EdgeStamp {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        // The edge band sees matched and unmatched routes alike — resolve
        // the error branch so a 404 is observed (and stamped) too.
        let mut resp = next.run(req).await.unwrap_or_else(|err| err.into_response());
        EDGE_EVENTS.lock().await.push("edge");
        resp.headers_mut()
            .insert("x-edge", "hit".parse().expect("static header value"));
        Ok(resp)
    }
}

#[injectable]
#[derive(Default)]
struct ScopedProbe;

impl Layer for ScopedProbe {}

#[async_trait]
impl Interceptor for ScopedProbe {
    async fn intercept(&self, req: Request, next: Next<'_>) -> Result<Response> {
        let resp = next.run(req).await?;
        EDGE_EVENTS.lock().await.push("scoped");
        Ok(resp)
    }
}

#[controller(path = "/edge")]
struct EdgeController;

#[routes]
impl EdgeController {
    #[get("/probe")]
    #[use_interceptors(ScopedProbe)]
    async fn probe(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [EdgeStamp, ScopedProbe, EdgeController])]
struct EdgeModule;

#[tokio::test]
async fn infra_interceptor_mounts_at_the_transport_edge_and_is_not_a_provider() {
    let _gate = GATE.lock().await;
    EDGE_EVENTS.lock().await.clear();

    let app = TestApp::for_module::<EdgeModule>().await.expect("boots");

    // A matched route: the scoped interceptor completes inside, the infra
    // wrap completes outside — band nesting matches the CLAUDE.md table.
    let resp = app.http().get("/edge/probe").send().await;
    resp.assert_status_is_ok();
    resp.assert_header("x-edge", "hit");
    assert_eq!(
        *EDGE_EVENTS.lock().await,
        vec!["scoped", "edge"],
        "infra band wraps outside the per-route interceptor pool",
    );

    // An unmatched path: the infra band still sees (and stamps) the 404 —
    // the pool interceptors never run for it.
    EDGE_EVENTS.lock().await.clear();
    let resp = app.http().get("/edge/nowhere").send().await;
    resp.assert_status(StatusCode::NOT_FOUND);
    resp.assert_header("x-edge", "hit");
    assert_eq!(*EDGE_EVENTS.lock().await, vec!["edge"]);

    // `#[interceptor]` mounts infrastructure; it must not register the type
    // as a resolvable provider.
    assert!(
        app.container().get::<EdgeStamp>().is_none(),
        "an infra interceptor is not a provider",
    );
}
