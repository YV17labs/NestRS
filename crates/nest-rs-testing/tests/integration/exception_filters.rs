//! Exception filters on the HTTP pipeline: typed dispatch at the three scopes
//! (global, controller, method), the closest scope first, `TypeId` dedup,
//! fallthrough when a filter does not match, a catch-all shaping what the
//! router answers itself and a self-mount's own error, and a failed unit
//! staying failed however a filter answered it.

use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{
    Caught, ExceptionFilter, Interceptor, Layer, Next, Outcome, UnitContext, async_trait,
    exception_filter, injectable, interceptor, module,
};
use nest_rs_graphql::{GraphqlModule, operations, resolver};
use nest_rs_http::{
    HeaderName, HeaderValue, HttpView, IntoResponse, ProblemDetails, StatusCode, controller, routes,
};
use nest_rs_testing::TestApp;
use tokio::sync::Mutex;

static DOMAIN_CATCH_COUNTER: AtomicUsize = AtomicUsize::new(0);
static GATE: Mutex<()> = Mutex::const_new(());

fn reset_counter() {
    DOMAIN_CATCH_COUNTER.store(0, Ordering::SeqCst);
}

fn catches() -> usize {
    DOMAIN_CATCH_COUNTER.load(Ordering::SeqCst)
}

#[derive(Debug, thiserror::Error)]
#[error("domain failure: {0}")]
pub(crate) struct DomainError(pub String);

#[derive(Debug, thiserror::Error)]
#[error("infrastructure failure: {0}")]
pub(crate) struct InfraError(pub String);

/// Without it, a `Result<_, DomainError>` return fails with an `E0277` on
/// `IntoResult`. The status is the **default** a filter replaces.
impl nest_rs_http::ResponseError for DomainError {
    fn status(&self) -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// Answer the unit with the `status` problem saying `detail`, through its
/// view.
fn answer(cx: &mut UnitContext<'_>, status: StatusCode, detail: String) -> Caught {
    let problem = ProblemDetails::from_status(status).with_detail(detail);
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
struct DomainErrorFilter;

impl Layer for DomainErrorFilter {}

#[async_trait]
impl ExceptionFilter for DomainErrorFilter {
    type Exception = DomainError;

    async fn catch(&self, err: &DomainError, cx: &mut UnitContext<'_>) -> Caught {
        DOMAIN_CATCH_COUNTER.fetch_add(1, Ordering::SeqCst);
        answer(
            cx,
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("caught: {err}"),
        )
    }
}

/// Every failure, whatever its error: a crate-wide envelope.
#[injectable]
#[derive(Default)]
struct CatchAll;

impl Layer for CatchAll {}

#[async_trait]
impl ExceptionFilter for CatchAll {
    type Exception = dyn Error + Send + Sync;

    async fn catch(&self, _: &Self::Exception, cx: &mut UnitContext<'_>) -> Caught {
        answer(cx, StatusCode::IM_A_TEAPOT, "shaped".to_owned())
    }
}

#[controller(path = "/global")]
struct GlobalScope;

#[routes]
impl GlobalScope {
    #[get("/domain")]
    async fn fail_domain_global(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::new(
            DomainError("bad input".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }

    #[get("/infra")]
    async fn fail_infra_global(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::new(
            InfraError("db down".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

#[controller(path = "/ctrl")]
#[use_exception_filters(DomainErrorFilter)]
struct ControllerScope;

#[routes]
impl ControllerScope {
    #[get("/domain")]
    async fn fail_domain_ctrl(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::new(
            DomainError("at controller scope".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

#[controller(path = "/method")]
struct MethodScope;

#[routes]
impl MethodScope {
    #[get("/domain")]
    #[use_exception_filters(DomainErrorFilter)]
    async fn fail_domain_method(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::new(
            DomainError("at method scope".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

#[controller(path = "/dup-global-method")]
struct DupGlobalMethod;

#[routes]
impl DupGlobalMethod {
    #[get("/domain")]
    #[use_exception_filters(DomainErrorFilter)]
    async fn fail_domain_dup(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::new(
            DomainError("dup".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

#[controller(path = "/dup-ctrl-method")]
#[use_exception_filters(DomainErrorFilter)]
struct DupCtrlMethod;

#[routes]
impl DupCtrlMethod {
    #[get("/domain")]
    #[use_exception_filters(DomainErrorFilter)]
    async fn fail_domain_ctrl_method(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::new(
            DomainError("dup".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

/// `Result<T, DomainError>` straight out of the handler, no hand-built
/// `HttpError`.
#[controller(path = "/ordinary")]
struct OrdinaryRaise;

#[routes]
impl OrdinaryRaise {
    #[get("/filtered")]
    #[use_exception_filters(DomainErrorFilter)]
    async fn filtered(&self) -> Result<&'static str, DomainError> {
        Err(DomainError("boom".into()))
    }

    #[get("/unfiltered")]
    async fn unfiltered(&self) -> Result<&'static str, DomainError> {
        Err(DomainError("boom".into()))
    }
}

/// A catch-all on the controller, a typed filter on one route.
#[controller(path = "/closest")]
#[use_exception_filters(CatchAll)]
struct ClosestScope;

#[routes]
impl ClosestScope {
    #[get("/typed")]
    #[use_exception_filters(DomainErrorFilter)]
    async fn typed(&self) -> Result<&'static str, DomainError> {
        Err(DomainError("closest".into()))
    }

    #[get("/other")]
    #[use_exception_filters(DomainErrorFilter)]
    async fn other(&self) -> nest_rs_http::Result<&'static str> {
        Err(nest_rs_http::HttpError::new(
            InfraError("not the route's".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ))
    }
}

#[module(providers = [
    DomainErrorFilter,
    CatchAll,
    GlobalScope,
    ControllerScope,
    MethodScope,
    DupGlobalMethod,
    DupCtrlMethod,
    OrdinaryRaise,
    ClosestScope,
])]
struct ExceptionFiltersModule;

#[tokio::test]
async fn exception_filter_at_global_scope_catches_typed_error() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::builder()
        .module::<ExceptionFiltersModule>()
        .use_exception_filters_global([exception_filter::<DomainErrorFilter>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/global/domain").send().await;
    resp.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(catches(), 1);
}

#[tokio::test]
async fn exception_filter_at_controller_scope_catches_typed_error() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::for_module::<ExceptionFiltersModule>()
        .await
        .expect("boots");

    let resp = app.http().get("/ctrl/domain").send().await;
    resp.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(catches(), 1);
}

#[tokio::test]
async fn exception_filter_at_method_scope_catches_typed_error() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::for_module::<ExceptionFiltersModule>()
        .await
        .expect("boots");

    let resp = app.http().get("/method/domain").send().await;
    resp.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(catches(), 1);
}

#[tokio::test]
async fn raising_the_error_the_ordinary_way_reaches_the_filter() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::for_module::<ExceptionFiltersModule>()
        .await
        .expect("boots");

    let filtered = app.http().get("/ordinary/filtered").send().await;
    filtered.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(detail(filtered).await, "caught: domain failure: boom");
    assert_eq!(catches(), 1);

    let unfiltered = app.http().get("/ordinary/unfiltered").send().await;
    unfiltered.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(catches(), 1, "the unfiltered route reaches no filter");
}

#[tokio::test]
async fn unmatched_error_falls_through_to_default_handler() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::builder()
        .module::<ExceptionFiltersModule>()
        .use_exception_filters_global([exception_filter::<DomainErrorFilter>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/global/infra").send().await;
    resp.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(catches(), 0, "DomainErrorFilter must not catch InfraError");
}

#[tokio::test]
async fn same_filter_global_and_method_runs_once() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::builder()
        .module::<ExceptionFiltersModule>()
        .use_exception_filters_global([exception_filter::<DomainErrorFilter>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/dup-global-method/domain").send().await;
    resp.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        catches(),
        1,
        "TypeId dedup: global + method redeclaration must still catch once",
    );
}

#[tokio::test]
async fn same_filter_at_all_three_scopes_catches_once() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::builder()
        .module::<ExceptionFiltersModule>()
        .use_exception_filters_global([exception_filter::<DomainErrorFilter>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/dup-ctrl-method/domain").send().await;
    resp.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        catches(),
        1,
        "global + controller + method declaration still catches exactly once",
    );
}

#[tokio::test]
async fn a_typed_filter_at_the_route_answers_before_a_catch_all_on_its_controller() {
    let _gate = GATE.lock().await;
    reset_counter();

    let app = TestApp::for_module::<ExceptionFiltersModule>()
        .await
        .expect("boots");

    let typed = app.http().get("/closest/typed").send().await;
    typed.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(catches(), 1);

    let other = app.http().get("/closest/other").send().await;
    other.assert_status(StatusCode::IM_A_TEAPOT);
    assert_eq!(detail(other).await, "shaped");
    assert_eq!(catches(), 1, "the route's typed filter declined it");
}

/// Stamps every answer of the unit stage it runs at.
#[interceptor(stage = unit)]
#[derive(Default)]
struct Observed;

impl Layer for Observed {}

#[async_trait]
impl Interceptor for Observed {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        let outcome = next.run(cx).await;
        let failed = outcome.is_err();
        if let Some(response) = HttpView::of(cx)?.response_mut() {
            response.headers_mut().insert(
                HeaderName::from_static("x-observed"),
                HeaderValue::from_static(if failed { "failed" } else { "ok" }),
            );
        }
        outcome
    }
}

#[module(providers = [CatchAll, Observed])]
struct ObservedModule;

#[tokio::test]
async fn a_404_is_shaped_by_a_global_catch_all_and_observed_at_the_unit_stage() {
    let app = TestApp::builder()
        .module::<ObservedModule>()
        .use_exception_filters_global([exception_filter::<CatchAll>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().get("/nowhere").send().await;
    resp.assert_status(StatusCode::IM_A_TEAPOT);
    resp.assert_header("x-observed", "failed");
    assert_eq!(detail(resp).await, "shaped");
}

#[resolver]
struct PingResolver;

#[operations]
impl PingResolver {
    #[query]
    #[public]
    async fn ping(&self) -> i32 {
        0
    }
}

#[module(
    imports = [GraphqlModule::for_root(nest_rs_graphql::GraphqlConfig {
        max_batch_size: 1,
        ..Default::default()
    })],
    providers = [CatchAll, PingResolver],
)]
struct GraphqlBatchModule;

#[tokio::test]
async fn a_global_catch_all_shapes_a_self_mounts_own_http_error() {
    let app = TestApp::builder()
        .module::<GraphqlBatchModule>()
        .use_exception_filters_global([exception_filter::<CatchAll>()])
        .build()
        .await
        .expect("boots");

    let one = serde_json::json!({ "query": "{ ping }" });
    let resp = app
        .http()
        .post("/graphql")
        .body_json(&serde_json::json!([one, one]))
        .send()
        .await;
    resp.assert_status(StatusCode::IM_A_TEAPOT);
    assert_eq!(detail(resp).await, "shaped");
}
