//! Layer-pool invariant: a guard declared at any combination of scopes
//! executes **exactly once** per request; a global interceptor runs after the
//! guards, so a denial never reaches it; a unit an exception filter answered
//! stays failed, so its data rolls back.

use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{
    Caught, ExceptionFilter, Interceptor, Layer, Next, Outcome, UnitContext, injectable,
    interceptor, module,
};
use nest_rs_guards::{Denial, Guard, GuardFor, guard};
use nest_rs_http::{
    HeaderName, HeaderValue, HttpView, Response, StatusCode, async_trait, controller, routes,
};
use nest_rs_testing::TestApp;
use tokio::sync::Mutex;

static COUNTER: AtomicUsize = AtomicUsize::new(0);
static GATE: Mutex<()> = Mutex::const_new(());

fn reset_counter() {
    COUNTER.store(0, Ordering::SeqCst);
}

fn counter() -> usize {
    COUNTER.load(Ordering::SeqCst)
}

/// Counts every execution, then admits the request.
#[injectable]
#[derive(Default)]
struct CountingGuard;

impl Layer for CountingGuard {}

#[async_trait]
impl Guard for CountingGuard {
    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        COUNTER.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for CountingGuard {}

#[controller(path = "/pool")]
#[use_guards(CountingGuard)]
struct ControllerGuarded;

#[routes]
impl ControllerGuarded {
    #[get("/c")]
    async fn c(&self) -> &'static str {
        "ok"
    }

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
    #[get("/m")]
    #[use_guards(CountingGuard)]
    async fn m(&self) -> &'static str {
        "ok"
    }

    // Used only under a global guard.
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

static EDGE_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn reset_edge_counter() {
    EDGE_COUNTER.store(0, Ordering::SeqCst);
}

fn edge_counter() -> usize {
    EDGE_COUNTER.load(Ordering::SeqCst)
}

/// Counts every unit it runs around, then forwards.
#[injectable]
#[derive(Default)]
struct EdgeObserver;

impl Layer for EdgeObserver {}

#[async_trait]
impl Interceptor for EdgeObserver {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        EDGE_COUNTER.fetch_add(1, Ordering::SeqCst);
        next.run(cx).await
    }
}

/// Answers a handler's error with a *success*.
#[injectable]
#[derive(Default)]
struct MapToOk;

impl Layer for MapToOk {}

#[async_trait]
impl ExceptionFilter for MapToOk {
    type Exception = dyn Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, cx: &mut UnitContext<'_>) -> Caught {
        match HttpView::of(cx) {
            Ok(view) => Caught::Answered(view.respond(Response::builder().body("recovered"))),
            Err(_) => Caught::Declined,
        }
    }
}

/// Says, at the unit stage, whether the unit failed, whatever it answered.
#[nest_rs_core::interceptor(stage = unit)]
#[derive(Default)]
struct UnitOutcome;

impl Layer for UnitOutcome {}

#[async_trait]
impl Interceptor for UnitOutcome {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let outcome = next.run(cx).await;
        let said = if outcome.is_ok() { "ok" } else { "failed" };
        if let Some(response) = HttpView::of(cx)?.response_mut() {
            response.headers_mut().insert(
                HeaderName::from_static("x-unit"),
                HeaderValue::from_static(said),
            );
        }
        outcome
    }
}

#[injectable]
#[derive(Default)]
struct DenyAll;

impl Layer for DenyAll {}

#[async_trait]
impl Guard for DenyAll {
    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Err(Denial::forbidden("denied"))
    }
}

impl GuardFor<nest_rs_http::HttpView> for DenyAll {}

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
    #[use_exception_filters(MapToOk)]
    async fn boom(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::from_status(
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

#[module(providers = [EdgeObserver, MapToOk, UnitOutcome, DenyAll, EdgeController])]
struct EdgeModule;

#[tokio::test]
async fn a_global_interceptor_never_sees_a_guard_denial() {
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
        0,
        "the guards answer before the handler stage, which a global interceptor runs at",
    );
}

#[tokio::test]
async fn a_unit_a_filter_answered_with_a_success_stays_failed() {
    let _gate = GATE.lock().await;

    let app = TestApp::for_module::<EdgeModule>().await.expect("boots");

    let resp = app.http().get("/edge/boom").send().await;
    resp.assert_status_is_ok();
    resp.assert_header("x-unit", "failed");
    resp.assert_text("recovered").await;
}
