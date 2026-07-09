//! URI versioning (`#[controller(version = "1")]`) and exception filters
//! (`#[use_filters]`), end-to-end through the HTTP harness. Also pins the
//! cross-scope TypeId dedup contract: a filter declared at any combination of
//! global / controller / method scopes is composed through `compose_chain` by
//! the per-route pool and executes exactly once.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, injectable, module};
use nest_rs_filters::{Filter, RequestSnapshot, filter};
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;
use poem::{Error, Response};
use tokio::sync::Mutex;

#[injectable]
#[derive(Default)]
struct TeapotFilter;

impl Layer for TeapotFilter {}

#[async_trait]
impl Filter for TeapotFilter {
    async fn filter(&self, _req: &RequestSnapshot, _error: Error) -> Response {
        Response::builder()
            .status(StatusCode::IM_A_TEAPOT)
            .body("filtered")
    }
}

#[controller(path = "/widgets", version = "1")]
struct WidgetController;

#[routes]
impl WidgetController {
    #[get("/")]
    async fn list(&self) -> &'static str {
        "widgets"
    }

    #[get("/boom")]
    #[use_filters(TeapotFilter)]
    async fn boom(&self) -> poem::Result<&'static str> {
        Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    }

    #[get("/raw-boom")]
    async fn raw_boom(&self) -> poem::Result<&'static str> {
        Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

#[controller(path = "/gadgets")]
#[use_filters(TeapotFilter)]
struct GadgetController;

#[routes]
impl GadgetController {
    #[get("/boom")]
    async fn gadget_boom(&self) -> poem::Result<&'static str> {
        Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

#[module(providers = [TeapotFilter, WidgetController, GadgetController])]
struct WidgetModule;

#[tokio::test]
async fn versioned_controller_is_served_under_v_prefix() {
    let app = TestApp::for_module::<WidgetModule>().await.expect("boots");
    let resp = app.http().get("/v1/widgets").send().await;
    resp.assert_status_is_ok();
    resp.assert_text("widgets").await;
}

#[tokio::test]
async fn unversioned_path_is_not_mounted() {
    let app = TestApp::for_module::<WidgetModule>().await.expect("boots");
    let resp = app.http().get("/widgets").send().await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn per_route_filter_maps_the_error() {
    let app = TestApp::for_module::<WidgetModule>().await.expect("boots");
    let resp = app.http().get("/v1/widgets/boom").send().await;
    resp.assert_status(StatusCode::IM_A_TEAPOT);
    resp.assert_text("filtered").await;
}

#[tokio::test]
async fn route_without_filter_uses_default_error() {
    let app = TestApp::for_module::<WidgetModule>().await.expect("boots");
    let resp = app.http().get("/v1/widgets/raw-boom").send().await;
    resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn controller_level_filter_maps_errors_without_a_per_route_filter() {
    let app = TestApp::for_module::<WidgetModule>().await.expect("boots");
    let resp = app.http().get("/gadgets/boom").send().await;
    resp.assert_status(StatusCode::IM_A_TEAPOT);
    resp.assert_text("filtered").await;
}

// --- TypeId dedup across scopes ---------------------------------------------
//
// The filter under test ([`CountingFilter`]) returns a fixed body and bumps
// a process-global counter every call. Tests share that counter, so a
// `tokio::sync::Mutex` serializes them — `cargo nextest` parallelizes by
// default.

static FILTER_COUNTER: AtomicUsize = AtomicUsize::new(0);
static GATE: Mutex<()> = Mutex::const_new(());

fn reset_filter_counter() {
    FILTER_COUNTER.store(0, Ordering::SeqCst);
}

fn filter_calls() -> usize {
    FILTER_COUNTER.load(Ordering::SeqCst)
}

#[injectable]
#[derive(Default)]
struct CountingFilter;

impl Layer for CountingFilter {}

#[async_trait]
impl Filter for CountingFilter {
    async fn filter(&self, _req: &RequestSnapshot, _error: Error) -> Response {
        FILTER_COUNTER.fetch_add(1, Ordering::SeqCst);
        Response::builder()
            .status(StatusCode::IM_A_TEAPOT)
            .body("counted")
    }
}

#[controller(path = "/dup-global-ctrl")]
#[use_filters(CountingFilter)]
struct DupGlobalCtrl;

#[routes]
impl DupGlobalCtrl {
    #[get("/boom")]
    async fn dup_global_ctrl_boom(&self) -> poem::Result<&'static str> {
        Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

#[controller(path = "/dup-global-method")]
struct DupGlobalMethod;

#[routes]
impl DupGlobalMethod {
    #[get("/boom")]
    #[use_filters(CountingFilter)]
    async fn dup_global_method_boom(&self) -> poem::Result<&'static str> {
        Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

#[controller(path = "/dup-ctrl-method")]
#[use_filters(CountingFilter)]
struct DupCtrlMethod;

#[routes]
impl DupCtrlMethod {
    #[get("/boom")]
    #[use_filters(CountingFilter)]
    async fn dup_ctrl_method_boom(&self) -> poem::Result<&'static str> {
        Err(Error::from_status(StatusCode::INTERNAL_SERVER_ERROR))
    }
}

#[module(providers = [
    CountingFilter,
    DupGlobalCtrl,
    DupGlobalMethod,
    DupCtrlMethod,
])]
struct DedupFilterModule;

#[tokio::test]
async fn same_filter_global_and_controller_runs_once() {
    let _gate = GATE.lock().await;
    reset_filter_counter();

    // Global seeding + a redeclaration on the controller. The per-route pool
    // composes global + controller and dedups by TypeId — broadest (global)
    // wins, so the filter runs once.
    let app = TestApp::builder()
        .module::<DedupFilterModule>()
        .use_filters_global([filter::<CountingFilter>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/dup-global-ctrl/boom").send().await;
    resp.assert_status(StatusCode::IM_A_TEAPOT);
    assert_eq!(
        filter_calls(),
        1,
        "TypeId dedup: global + controller redeclaration must execute once",
    );
}

#[tokio::test]
async fn same_filter_global_and_method_runs_once() {
    let _gate = GATE.lock().await;
    reset_filter_counter();

    // Same shape: the method-scope wrap is skipped at mount time when
    // the TypeId is already in `FilterSpecs`.
    let app = TestApp::builder()
        .module::<DedupFilterModule>()
        .use_filters_global([filter::<CountingFilter>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/dup-global-method/boom").send().await;
    resp.assert_status(StatusCode::IM_A_TEAPOT);
    assert_eq!(
        filter_calls(),
        1,
        "TypeId dedup: global + method redeclaration must execute once",
    );
}

#[tokio::test]
async fn same_filter_controller_and_method_runs_once() {
    let _gate = GATE.lock().await;
    reset_filter_counter();

    // Controller and method both declare the filter. The per-route pool
    // composes them through `compose_chain` and dedups by `TypeId` (broadest
    // scope wins) — one filter wrap on the error path, one execution.
    let app = TestApp::for_module::<DedupFilterModule>()
        .await
        .expect("boots");

    let resp = app.http().get("/dup-ctrl-method/boom").send().await;
    resp.assert_status(StatusCode::IM_A_TEAPOT);
    assert_eq!(
        filter_calls(),
        1,
        "controller + method filter declaration dedups to a single execution",
    );
}

#[tokio::test]
async fn same_filter_at_all_three_scopes_runs_once() {
    let _gate = GATE.lock().await;
    reset_filter_counter();

    // Global + controller + method — the broadest (global) wins and maps the
    // error at the transport edge; both narrower redeclarations are dropped.
    let app = TestApp::builder()
        .module::<DedupFilterModule>()
        .use_filters_global([filter::<CountingFilter>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/dup-ctrl-method/boom").send().await;
    resp.assert_status(StatusCode::IM_A_TEAPOT);
    assert_eq!(
        filter_calls(),
        1,
        "global + controller + method filter declaration still executes exactly once",
    );
}
