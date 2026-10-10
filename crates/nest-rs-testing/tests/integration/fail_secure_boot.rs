//! Boot-time fail-secure contract of the layer pool: a global layer spec whose
//! provider was never registered, a controller guard no module provides, or two
//! controllers on one prefix, fail the boot.

use nest_rs_core::{Layer, injectable, module, target};
use nest_rs_exception_filters::{ExceptionFilter, exception_filter};
use nest_rs_filters::{Filter, RequestSnapshot, filter};
use nest_rs_guards::{Denial, Guard, HttpGuard, guard};
use nest_rs_http::{async_trait, controller, routes};
use nest_rs_interceptors::{Interceptor, Next, interceptor};
use nest_rs_testing::TestApp;
use poem::{IntoResponse, Request, Response};

/// Registered as a provider: the resolvable control case.
#[injectable]
#[derive(Default)]
struct WiredGuard;

impl Layer for WiredGuard {}

#[async_trait]
impl Guard for WiredGuard {
    async fn check_http(&self, _req: &mut Request) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl HttpGuard for WiredGuard {}

/// Deliberately **not** listed in any module's providers.
#[injectable]
#[derive(Default)]
struct GhostGuard;

impl Layer for GhostGuard {}

#[async_trait]
impl Guard for GhostGuard {
    async fn check_http(&self, _req: &mut Request) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl HttpGuard for GhostGuard {}

/// Deliberately **not** listed in any module's providers.
#[injectable]
#[derive(Default)]
struct GhostInterceptor;

impl Layer for GhostInterceptor {}

#[async_trait]
impl Interceptor for GhostInterceptor {
    async fn intercept(&self, req: Request, next: Next<'_>) -> poem::Result<Response> {
        next.run(req).await
    }
}

/// Deliberately **not** listed in any module's providers.
#[injectable]
#[derive(Default)]
struct GhostFilter;

impl Layer for GhostFilter {}

#[async_trait]
impl Filter for GhostFilter {
    async fn filter(&self, _req: &RequestSnapshot, error: poem::Error) -> Response {
        error.into_response()
    }
}

/// Deliberately **not** listed in any module's providers.
#[injectable]
#[derive(Default)]
struct GhostExceptionFilter;

impl Layer for GhostExceptionFilter {}

#[async_trait]
impl ExceptionFilter for GhostExceptionFilter {
    type Exception = std::fmt::Error;

    async fn catch(&self, _err: std::fmt::Error) -> Response {
        poem::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
}

#[module(providers = [WiredGuard])]
struct GuardOnlyModule;

/// `TestApp` is not `Debug`, so unwrap the error arm by hand.
fn boot_error(result: anyhow::Result<TestApp>, expectation: &str) -> anyhow::Error {
    match result {
        Ok(_) => panic!("{expectation}"),
        Err(err) => err,
    }
}

#[tokio::test]
async fn an_unresolvable_global_guard_fails_boot() {
    let result = TestApp::builder()
        .module::<GuardOnlyModule>()
        .use_guards_global([guard::<GhostGuard>()])
        .build()
        .await;
    let err = boot_error(
        result,
        "a global guard with no provider must fail boot, not silently drop",
    );
    assert!(
        err.to_string().contains("GhostGuard"),
        "the error names the unresolvable guard: {err}",
    );
}

#[tokio::test]
async fn an_unresolvable_global_interceptor_fails_boot() {
    let result = TestApp::builder()
        .module::<GuardOnlyModule>()
        .use_interceptors_global([interceptor::<GhostInterceptor>()])
        .build()
        .await;
    let err = boot_error(
        result,
        "a global interceptor with no provider must fail boot",
    );
    assert!(
        format!("{err:#}").contains("GhostInterceptor"),
        "the error names the unresolvable interceptor: {err:#}",
    );
}

#[tokio::test]
async fn an_app_serving_no_http_still_fails_on_an_unresolvable_global_interceptor() {
    let result = TestApp::builder()
        .module::<GuardOnlyModule>()
        .use_interceptors_global([interceptor::<GhostInterceptor>()])
        .build_headless()
        .await;
    let Err(err) = result else {
        panic!("a worker would boot with a global interceptor silently dropped");
    };
    assert!(
        format!("{err:#}").contains("GhostInterceptor"),
        "the error names the unresolvable interceptor: {err:#}",
    );
}

#[tokio::test]
async fn an_app_serving_no_http_still_fails_on_an_unresolvable_global_filter() {
    let result = TestApp::builder()
        .module::<GuardOnlyModule>()
        .use_filters_global([filter::<GhostFilter>()])
        .build_headless()
        .await;
    let Err(err) = result else {
        panic!("a worker would boot with a global filter silently dropped");
    };
    let chain = format!("{err:#}");
    assert!(
        chain.contains("GhostFilter") && chain.contains("nest_rs::filters::global"),
        "the error names the unresolvable filter and the wiring refusing it: {chain}",
    );
}

#[tokio::test]
async fn an_app_serving_no_http_still_fails_on_an_unresolvable_global_exception_filter() {
    let result = TestApp::builder()
        .module::<GuardOnlyModule>()
        .use_exception_filters_global([exception_filter::<GhostExceptionFilter>()])
        .build_headless()
        .await;
    let Err(err) = result else {
        panic!("a worker would boot with a global exception filter silently dropped");
    };
    let chain = format!("{err:#}");
    assert!(
        chain.contains("GhostExceptionFilter")
            && chain.contains("nest_rs::exception_filters::global"),
        "the error names the unresolvable exception filter and the wiring refusing it: {chain}",
    );
}

/// Gated by a guard no module provides.
#[controller(path = "/ghost-gated")]
#[use_guards(GhostGuard)]
struct GhostGatedController;

#[routes]
impl GhostGatedController {
    #[get("/")]
    async fn read(&self) -> &'static str {
        "unreachable"
    }
}

#[module(providers = [GhostGatedController])]
struct GhostGatedModule;

#[tokio::test]
async fn a_controller_guard_no_module_provides_fails_the_boot_naming_it_and_the_controller() {
    // Built by hand, so the access graph does not refuse first.
    let container = nest_rs_core::Container::builder()
        .import::<GhostGatedModule>()
        .build();
    let mut transport = nest_rs_http::HttpTransport::default();
    let Err(err) = nest_rs_core::Transport::configure(&mut transport, &container).await else {
        panic!("the controller would serve without the guard it declares");
    };
    let chain = format!("{err:#}");
    assert!(
        chain.contains("GhostGuard") && chain.contains("GhostGatedController"),
        "the refusal names the guard and the controller declaring it: {chain}",
    );
    assert!(
        chain.contains("no imported module provides it"),
        "…and the remedy: {chain}",
    );
}

// The imperative-mount cases live in `nest-rs-http/tests/integration/fail_secure.rs`.

/// A controller route with no guard and no `#[public]` marker: an *implicit*
/// access decision.
#[controller(path = "/")]
struct OpenController;

#[routes]
impl OpenController {
    #[post("/thing")]
    async fn create(&self) -> String {
        "made".into()
    }
}

#[module(providers = [OpenController])]
struct OpenModule;

#[tokio::test]
async fn an_unguarded_non_public_route_warns_but_boots_without_a_global_pool() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<OpenModule>()
        .await
        .expect("an implicit access decision is a warning, not a boot failure");
    app.http().post("/thing").send().await.assert_status_is_ok();

    let event = logs
        .find(target::LAYERS, "unguarded routes detected")
        .into_iter()
        .next()
        .expect("a route deciding access implicitly is named at boot");
    assert_eq!(event.level, "warn");
    assert!(
        event.field("routes").is_some_and(|r| r.contains("/thing")),
        "and the line names the routes, not just how many: a count sends the \
         reader looking through the whole table: {event:?}",
    );
    assert!(
        event.field("hint").is_some(),
        "with the remedy, since a warning that cannot be acted on is noise: {event:?}",
    );
}

/// Two controllers claiming the same prefix, on which poem would panic deep in
/// route assembly ("duplicate path: /dup/*--poem-rest").
#[controller(path = "/dup")]
struct FirstDupController;

#[routes]
impl FirstDupController {
    #[post("/")]
    #[public]
    async fn first(&self) -> String {
        "first".into()
    }
}

#[controller(path = "/dup")]
struct SecondDupController;

#[routes]
impl SecondDupController {
    #[post("/")]
    #[public]
    async fn second(&self) -> String {
        "second".into()
    }
}

#[module(providers = [FirstDupController, SecondDupController])]
struct DuplicatePrefixModule;

#[tokio::test]
async fn two_controllers_sharing_a_prefix_fail_boot_naming_both() {
    let result = TestApp::builder()
        .module::<DuplicatePrefixModule>()
        .build()
        .await;
    let err = boot_error(
        result,
        "two controllers on one prefix must fail boot, not panic inside poem",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("/dup")
            && msg.contains("FirstDupController")
            && msg.contains("SecondDupController"),
        "the error names the shared prefix and both controllers: {err}",
    );
}
