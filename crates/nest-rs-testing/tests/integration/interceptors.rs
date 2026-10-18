//! Interceptors on the HTTP pipeline: binding per handler and controller, the
//! guards and the class gate before every handler-stage interceptor, the
//! cross-scope `TypeId` dedup, the unit stage around the guards seeing what
//! the router answers itself, and a module's `#[interceptor]` being the
//! container's own provider.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{
    AmbientValue, Interceptor, Layer, Next, Outcome, UnitContext, async_trait, injectable,
    interceptor, module,
};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{HeaderName, HeaderValue, HttpView, StatusCode, controller, routes};
use nest_rs_testing::TestApp;
use tokio::sync::Mutex;

/// Stamp `name` on the answer the unit holds, once the rest of it ran.
fn stamp(cx: &mut UnitContext<'_>, name: &'static str) -> Result<(), Denial> {
    if let Some(response) = HttpView::of(cx)?.response_mut() {
        response.headers_mut().insert(
            HeaderName::from_static(name),
            HeaderValue::from_static("hit"),
        );
    }
    Ok(())
}

#[injectable]
#[derive(Default)]
struct Tracer;

impl Layer for Tracer {}

#[async_trait]
impl Interceptor for Tracer {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let outcome = next.run(cx).await;
        stamp(cx, "x-trace")?;
        outcome
    }
}

#[injectable]
#[derive(Default)]
struct DenyGuard;

impl Layer for DenyGuard {}

#[async_trait]
impl Guard for DenyGuard {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("denied"))
    }
}

impl GuardFor<HttpView> for DenyGuard {}

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
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        COUNTER.fetch_add(1, Ordering::SeqCst);
        next.run(cx).await
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

/// What a module's `#[interceptor]` counted, on the provider it depends on.
#[injectable]
#[derive(Default)]
struct Units {
    seen: AtomicUsize,
}

/// A module's handler-stage interceptor whose dependency its module lists
/// after it.
#[interceptor]
struct CountUnits {
    #[inject]
    units: Arc<Units>,
}

impl Layer for CountUnits {}

#[async_trait]
impl Interceptor for CountUnits {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        self.units.seen.fetch_add(1, Ordering::SeqCst);
        next.run(cx).await
    }
}

#[controller(path = "/counted")]
struct CountedController;

#[routes]
impl CountedController {
    #[get("/")]
    async fn counted(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [CountUnits, Units, CountedController])]
struct CountingModule;

#[tokio::test]
async fn a_modules_interceptor_is_the_containers_provider_whatever_order_it_is_listed_in() {
    let app = TestApp::for_module::<CountingModule>()
        .await
        .expect("an interceptor listed before its dependency boots");
    app.http()
        .get("/counted")
        .send()
        .await
        .assert_status_is_ok();
    let units = app.container().get::<Units>().expect("provided");
    assert_eq!(units.seen.load(Ordering::SeqCst), 1);
    assert!(
        app.container().get::<CountUnits>().is_some(),
        "an interceptor a module brings is a provider of its own"
    );
}

#[tokio::test]
async fn a_modules_interceptor_the_app_also_lists_runs_once() {
    let app = TestApp::builder()
        .module::<CountingModule>()
        .use_interceptors_global([interceptor::<CountUnits>()])
        .build()
        .await
        .expect("boots");
    app.http()
        .get("/counted")
        .send()
        .await
        .assert_status_is_ok();
    let units = app.container().get::<Units>().expect("provided");
    assert_eq!(
        units.seen.load(Ordering::SeqCst),
        1,
        "listed twice, run once"
    );
}

/// At the unit stage, around the guards: it sees every HTTP unit's answer.
#[interceptor(stage = unit)]
#[derive(Default)]
struct EdgeStamp;

impl Layer for EdgeStamp {}

#[async_trait]
impl Interceptor for EdgeStamp {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let outcome = next.run(cx).await;
        stamp(cx, "x-edge")?;
        outcome
    }
}

#[controller(path = "/edgeband")]
struct EdgeBandController;

#[routes]
impl EdgeBandController {
    #[get("/ok")]
    async fn ok(&self) -> &'static str {
        "ok"
    }

    #[get("/denied")]
    #[use_guards(DenyGuard)]
    async fn denied(&self) -> &'static str {
        "unreachable"
    }
}

#[module(providers = [EdgeStamp, Tracer, DenyGuard, EdgeBandController])]
struct EdgeBandModule;

#[tokio::test]
async fn a_unit_stage_interceptor_sees_matched_routes_denials_404s_and_405s() {
    let app = TestApp::for_module::<EdgeBandModule>()
        .await
        .expect("boots");

    let matched = app.http().get("/edgeband/ok").send().await;
    matched.assert_status_is_ok();
    matched.assert_header("x-edge", "hit");

    // The edge trims a trailing slash before routing.
    let trailing = app.http().get("/edgeband/ok/").send().await;
    trailing.assert_status_is_ok();
    trailing.assert_header("x-edge", "hit");

    let denied = app.http().get("/edgeband/denied").send().await;
    denied.assert_status(StatusCode::FORBIDDEN);
    denied.assert_header("x-edge", "hit");

    let bogus = app.http().get("/totally-bogus").send().await;
    bogus.assert_status(StatusCode::NOT_FOUND);
    bogus.assert_header("x-edge", "hit");

    let bogus_slashed = app.http().get("/totally-bogus/").send().await;
    bogus_slashed.assert_status(StatusCode::NOT_FOUND);
    bogus_slashed.assert_header("x-edge", "hit");

    let wrong_method = app.http().post("/edgeband/ok").send().await;
    wrong_method.assert_status(StatusCode::METHOD_NOT_ALLOWED);
    wrong_method.assert_header("x-edge", "hit");
}

#[tokio::test]
async fn a_global_interceptor_runs_after_the_guards_and_sees_no_404() {
    let app = TestApp::builder()
        .module::<EdgeBandModule>()
        .use_interceptors_global([interceptor::<Tracer>()])
        .build()
        .await
        .expect("boots");

    let matched = app.http().get("/edgeband/ok").send().await;
    matched.assert_status_is_ok();
    matched.assert_header("x-trace", "hit");

    let denied = app.http().get("/edgeband/denied").send().await;
    denied.assert_status(StatusCode::FORBIDDEN);
    denied.assert_header_is_not_exist("x-trace");

    let bogus = app.http().get("/totally-bogus").send().await;
    bogus.assert_status(StatusCode::NOT_FOUND);
    bogus.assert_header_is_not_exist("x-trace");
}

/// What the route declared, read back by a global interceptor.
#[derive(Debug)]
struct Audited(&'static str);

/// What a guard attached for the unit.
#[derive(Debug)]
struct Principal(&'static str);

impl AmbientValue for Principal {}

#[injectable]
#[derive(Default)]
struct AttachPrincipal;

impl Layer for AttachPrincipal {}

#[async_trait]
impl Guard for AttachPrincipal {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        cx.attach(Principal("alice"));
        Ok(())
    }
}

impl GuardFor<HttpView> for AttachPrincipal {}

/// Writes what the route declared and who the guards admitted onto the answer.
#[injectable]
#[derive(Default)]
struct Reader;

impl Layer for Reader {}

#[async_trait]
impl Interceptor for Reader {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let audited = cx
            .reflector()
            .get::<Audited>()
            .map_or("-", |audited| audited.0);
        let principal = cx
            .ambient::<Principal>()
            .map_or("-", |principal| principal.0);
        let outcome = next.run(cx).await;
        if let Some(response) = HttpView::of(cx)?.response_mut() {
            let read = format!("{audited} {principal}");
            response.headers_mut().insert(
                HeaderName::from_static("x-read"),
                HeaderValue::from_str(&read).expect("ascii"),
            );
        }
        outcome
    }
}

#[controller(path = "/read")]
#[use_guards(AttachPrincipal)]
struct ReadController;

#[routes]
impl ReadController {
    #[get("/")]
    #[meta(Audited("ledger"))]
    async fn read(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [AttachPrincipal, Reader, ReadController])]
struct ReadModule;

#[tokio::test]
async fn a_global_interceptor_reads_the_routes_declaration_and_what_its_guards_attached() {
    let app = TestApp::builder()
        .module::<ReadModule>()
        .use_interceptors_global([interceptor::<Reader>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/read").send().await;
    resp.assert_status_is_ok();
    resp.assert_header("x-read", "ledger alice");
}
