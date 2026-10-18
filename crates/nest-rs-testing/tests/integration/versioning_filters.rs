//! URI versioning (`#[controller(version = "1")]`) and a catch-all exception
//! filter (`#[use_exception_filters]` over `dyn Error + Send + Sync`), the
//! untyped mapper's successor, including the cross-scope `TypeId` dedup, and
//! what a header or media-type strategy refuses answered as the router's own
//! answers are.

use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{
    Caught, ExceptionFilter, Layer, UnitContext, exception_filter, injectable, module,
};
use nest_rs_http::{
    HttpView, IntoResponse, ProblemDetails, StatusCode, async_trait, controller, routes,
};
use nest_rs_testing::TestApp;
use tokio::sync::Mutex;

/// Answer the unit `418`, its problem saying `detail`.
fn answer(cx: &mut UnitContext<'_>, detail: &'static str) -> Caught {
    let problem = ProblemDetails::from_status(StatusCode::IM_A_TEAPOT).with_detail(detail);
    match HttpView::of(cx) {
        Ok(view) => Caught::Answered(view.respond(problem.into_response())),
        Err(_) => Caught::Declined,
    }
}

/// The `detail` the answer's problem says.
async fn detail(resp: nest_rs_testing::TestResponse) -> String {
    let body: serde_json::Value = resp.json().await.value().deserialize();
    body["detail"].as_str().unwrap_or_default().to_owned()
}

#[injectable]
#[derive(Default)]
struct TeapotFilter;

impl Layer for TeapotFilter {}

#[async_trait]
impl ExceptionFilter for TeapotFilter {
    type Exception = dyn Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, cx: &mut UnitContext<'_>) -> Caught {
        answer(cx, "filtered")
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
    #[use_exception_filters(TeapotFilter)]
    async fn boom(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::from_status(
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }

    #[get("/raw-boom")]
    async fn raw_boom(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::from_status(
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

#[controller(path = "/gadgets")]
#[use_exception_filters(TeapotFilter)]
struct GadgetController;

#[routes]
impl GadgetController {
    #[get("/boom")]
    async fn gadget_boom(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::from_status(
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
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
    assert_eq!(detail(resp).await, "filtered");
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
    assert_eq!(detail(resp).await, "filtered");
}

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
impl ExceptionFilter for CountingFilter {
    type Exception = dyn Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, cx: &mut UnitContext<'_>) -> Caught {
        FILTER_COUNTER.fetch_add(1, Ordering::SeqCst);
        answer(cx, "counted")
    }
}

#[controller(path = "/dup-global-ctrl")]
#[use_exception_filters(CountingFilter)]
struct DupGlobalCtrl;

#[routes]
impl DupGlobalCtrl {
    #[get("/boom")]
    async fn dup_global_ctrl_boom(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::from_status(
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

#[controller(path = "/dup-global-method")]
struct DupGlobalMethod;

#[routes]
impl DupGlobalMethod {
    #[get("/boom")]
    #[use_exception_filters(CountingFilter)]
    async fn dup_global_method_boom(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::from_status(
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

#[controller(path = "/dup-ctrl-method")]
#[use_exception_filters(CountingFilter)]
struct DupCtrlMethod;

#[routes]
impl DupCtrlMethod {
    #[get("/boom")]
    #[use_exception_filters(CountingFilter)]
    async fn dup_ctrl_method_boom(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::from_status(
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
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

    let app = TestApp::builder()
        .module::<DedupFilterModule>()
        .use_exception_filters_global([exception_filter::<CountingFilter>()])
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

    let app = TestApp::builder()
        .module::<DedupFilterModule>()
        .use_exception_filters_global([exception_filter::<CountingFilter>()])
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

    let app = TestApp::builder()
        .module::<DedupFilterModule>()
        .use_exception_filters_global([exception_filter::<CountingFilter>()])
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

/// What the version selector refuses, answered as the router's own `404`: a
/// unit the app's catch-all shapes and its unit stage sees.
mod refused_versions {
    use nest_rs_core::{Interceptor, Next, Outcome, interceptor};
    use nest_rs_http::{
        ApiVersioning, DEFAULT_VERSION_HEADER, HeaderName, HeaderValue, HttpConfig,
        MEDIA_TYPE_PARAM,
    };

    use super::*;

    /// Stamps every answer of the unit stage it runs at.
    #[interceptor(stage = unit)]
    #[derive(Default)]
    struct Seen;

    impl Layer for Seen {}

    #[async_trait]
    impl Interceptor for Seen {
        async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
            let outcome = next.run(cx).await;
            if let Some(response) = HttpView::of(cx)?.response_mut() {
                response.headers_mut().insert(
                    HeaderName::from_static("x-seen"),
                    HeaderValue::from_static("hit"),
                );
            }
            outcome
        }
    }

    #[module(providers = [TeapotFilter, Seen, WidgetController])]
    struct RefusalsModule;

    async fn app(versioning: ApiVersioning) -> TestApp {
        TestApp::builder()
            .module::<RefusalsModule>()
            .provide(HttpConfig {
                versioning,
                ..HttpConfig::default()
            })
            .use_exception_filters_global([exception_filter::<TeapotFilter>()])
            .build()
            .await
            .expect("boots")
    }

    /// How a client states `version` under `versioning`.
    fn stating(versioning: ApiVersioning, version: &str) -> (&'static str, String) {
        match versioning {
            ApiVersioning::MediaType => (
                "accept",
                format!("application/json; {MEDIA_TYPE_PARAM}={version}"),
            ),
            _ => (DEFAULT_VERSION_HEADER, version.to_owned()),
        }
    }

    #[tokio::test]
    async fn every_refusal_of_a_rewriting_strategy_is_shaped_and_seen() {
        for versioning in [ApiVersioning::Header, ApiVersioning::MediaType] {
            let app = app(versioning).await;
            let (name, malformed) = stating(versioning, "1@x");
            let (_, unserved) = stating(versioning, "9");
            for (case, resp) in [
                ("unrouted", app.http().get("/nowhere").send().await),
                ("the URI form", app.http().get("/v1/widgets").send().await),
                (
                    "a malformed version",
                    app.http()
                        .get("/widgets")
                        .header(name, &malformed)
                        .send()
                        .await,
                ),
                (
                    "a version no route serves",
                    app.http()
                        .get("/widgets")
                        .header(name, &unserved)
                        .send()
                        .await,
                ),
            ] {
                resp.assert_status(StatusCode::IM_A_TEAPOT);
                resp.assert_header("x-seen", "hit");
                assert_eq!(detail(resp).await, "filtered", "{versioning:?}: {case}");
            }
        }
    }
}
