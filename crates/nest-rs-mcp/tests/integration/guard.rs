//! The guards at the `/mcp` request — the global pool and the
//! `EndpointGuards<McpView, _>` the app's modules declare, run by the transport
//! in front of the endpoint — and what they refuse, as `src/guard.rs` renders
//! it.

use std::sync::{Arc, Mutex};

use nest_rs_core::{AmbientValue, Layer, Posture, UnitContext, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor, GuardPhase, guard};
use nest_rs_http::{EndpointGuards, HttpView, StatusCode, async_trait};
use nest_rs_mcp::{
    CallToolResult, ContentBlock, McpError, McpModule, McpOptions, McpView, ServerHandler, mcp,
    tool, tool_handler, tool_router,
};
use nest_rs_testing::mcp::{call_tool, initialize_request, post_message};
use nest_rs_testing::{LogCapture, TestApp};
use nest_rs_throttler::{ThrottlerConfig, ThrottlerGuard, ThrottlerModule};

#[mcp]
#[derive(Clone)]
struct PoolTool;

#[tool_router(allow_empty)]
impl PoolTool {}

#[tool_handler]
impl ServerHandler for PoolTool {}

// `ThrottlerModule` registers `ThrottlerGuard`; listing it too fails the boot.
#[module(imports = [ThrottlerModule::for_root(one_per_minute())], providers = [PoolTool])]
struct PoolModule;

fn one_per_minute() -> ThrottlerConfig {
    ThrottlerConfig {
        limit: Some(1),
        window_secs: Some(60),
        pseudonym_key: None,
    }
}

#[tokio::test]
async fn a_global_throttler_guard_rate_limits_mcp() {
    let app = TestApp::builder()
        .module::<PoolModule>()
        .use_guards_global([guard::<ThrottlerGuard>()])
        .build()
        .await
        .expect("boots with a global throttler and no endpoint guards");

    let first = app.http().post("/mcp").send().await;
    assert_ne!(
        first.0.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the first call is inside the 1/minute budget",
    );
    assert_ne!(
        first.0.status(),
        StatusCode::UNAUTHORIZED,
        "with a global pool seeded, `/mcp` is no longer deny-all",
    );

    let second = app.http().post("/mcp").send().await;
    second.assert_status(StatusCode::TOO_MANY_REQUESTS);
    assert!(
        second.0.headers().contains_key("retry-after"),
        "the pooled throttler's denial keeps its `Retry-After`, not a flattened 401",
    );
}

#[module(providers = [PoolTool])]
struct NoPoolModule;

/// With neither a pool nor endpoint guards, a gated `/mcp` refuses every
/// request: said once at boot, and filed per refusal so a missing wiring tells
/// itself apart from a refused caller.
#[tokio::test]
async fn an_empty_pool_and_no_endpoint_guards_deny_initialize() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<NoPoolModule>().await.expect("boots");

    post_message(app.http(), "/mcp", None, None, &initialize_request())
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    let announced = logs.expect_one(
        "nest_rs::layers",
        "a gated endpoint runs no guard at its request; it denies every request",
    );
    assert_eq!(announced.level, "warn");
    assert_eq!(announced.field("path").as_deref(), Some("/mcp"));
    let denied = logs.expect_one(
        "nest_rs::layers",
        "request denied: the endpoint runs no guard at its request",
    );
    assert_eq!(denied.field("path").as_deref(), Some("/mcp"));
}

#[tokio::test]
async fn an_empty_global_pool_leaves_mcp_deny_all() {
    let app = TestApp::builder()
        .module::<NoPoolModule>()
        .use_guards_global([])
        .build()
        .await
        .expect("boots");
    app.http()
        .post("/mcp")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

/// The `check_http` calls of the recording guards below, in the order they ran.
#[injectable]
#[derive(Default)]
struct Trail {
    seen: Mutex<Vec<&'static str>>,
}

impl Trail {
    fn take(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.seen.lock().expect("trail lock"))
    }
}

/// A guard admitting every request after writing its name on the [`Trail`],
/// naming the caller by it as the authentication guard does, so a session
/// opens for it.
macro_rules! recording_guard {
    ($name:ident, $label:literal) => {
        #[injectable]
        struct $name {
            #[inject]
            trail: Arc<Trail>,
        }

        impl Layer for $name {}

        #[async_trait]
        impl Guard for $name {
            async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
                self.trail.seen.lock().expect("trail lock").push($label);
                nest_rs_core::__private::set_actor_id($label);
                Ok(())
            }
        }

        impl GuardFor<HttpView> for $name {}
    };
}

recording_guard!(Pooled, "pooled");
recording_guard!(Declared, "declared");

#[module(providers = [
    Trail,
    Pooled,
    Declared,
    PoolTool,
    EndpointGuards<McpView, (Pooled, Declared)>,
])]
struct DeclaredModule;

/// The pool and the endpoint guards both run at the request, each once: a guard
/// in both runs where the pool lists it.
#[tokio::test]
async fn endpoint_guards_compose_with_the_pool() {
    let app = TestApp::builder()
        .module::<DeclaredModule>()
        .use_guards_global([guard::<Pooled>()])
        .build()
        .await
        .expect("boots");
    let trail = app.container().get::<Trail>().expect("the trail");

    post_message(app.http(), "/mcp", None, None, &initialize_request())
        .await
        .assert_status(StatusCode::OK);
    assert_eq!(trail.take(), ["pooled", "declared"]);
}

#[module(
    imports = [McpModule::for_root(McpOptions { posture: Posture::Public, ..Default::default() })],
    providers = [PoolTool, DenyEverything],
)]
struct OpenedModule;

/// Refuses every request.
#[injectable]
#[derive(Default)]
struct DenyEverything;

impl Layer for DenyEverything {}

#[async_trait]
impl Guard for DenyEverything {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("the pool refused"))
    }
}

impl GuardFor<HttpView> for DenyEverything {}

/// The app opens `/mcp` in code: no guard runs at its request, the global
/// pool's included.
#[tokio::test]
async fn mcp_options_public_opens_the_endpoint() {
    let app = TestApp::builder()
        .module::<OpenedModule>()
        .use_guards_global([guard::<DenyEverything>()])
        .build()
        .await
        .expect("boots");

    post_message(app.http(), "/mcp", None, None, &initialize_request())
        .await
        .assert_status(StatusCode::OK);
}

#[module(imports = [OpenedModule, crate::open_mcp()])]
struct OpenedTwiceApp;

/// One opening, one site: a second import opening `/mcp` is refused naming
/// both, even though the two agree, before any factory runs.
#[tokio::test]
async fn two_imports_opening_the_endpoint_fail_the_boot_naming_both() {
    let Err(refused) = nest_rs_core::App::builder()
        .module::<OpenedTwiceApp>()
        .build()
        .await
    else {
        panic!("an opening declared at two sites must not boot");
    };
    let contested = refused
        .downcast_ref::<nest_rs_core::ContestedDeclarationError>()
        .unwrap_or_else(|| panic!("refused as a contest: {refused:#}"));
    assert_eq!(
        (contested.first.as_str(), contested.second.as_str()),
        (
            "`McpModule::for_root(..)` at `imports[0]` of `OpenedModule`",
            "`crate::open_mcp(..)` at `imports[1]` of `OpenedTwiceApp`",
        )
    );
    assert!(
        contested.remedy.contains("`posture: Posture::Public`"),
        "the remedy says which field to set once: {}",
        contested.remedy,
    );
}

#[module(
    imports = [McpModule::for_root(McpOptions { posture: Posture::Public, ..Default::default() })],
    providers = [Trail, Declared, PoolTool, EndpointGuards<McpView, (Declared,)>],
)]
struct OpenedAndDeclaredModule;

/// An opening would drop the guards a module declares for the endpoint: the
/// boot refuses the contradiction, naming the declaration.
#[tokio::test]
async fn an_opened_endpoint_a_module_declares_guards_for_fails_the_boot() {
    let Err(refused) = TestApp::for_module::<OpenedAndDeclaredModule>().await else {
        panic!("an opening must not drop declared guards silently");
    };
    let said = format!("{refused:#}");
    assert!(
        said.contains("declared public") && said.contains("EndpointGuards"),
        "{said}",
    );
}

/// Authorizes after an authentication guard, declaring it.
#[injectable]
#[derive(Default)]
struct Authorizing;

impl Layer for Authorizing {}

impl Guard for Authorizing {
    fn phase(&self) -> GuardPhase {
        GuardPhase::Authorization
    }
}

impl GuardFor<HttpView> for Authorizing {}

/// Authenticates, declaring it.
#[injectable]
#[derive(Default)]
struct Authenticating;

impl Layer for Authenticating {}

impl Guard for Authenticating {
    fn phase(&self) -> GuardPhase {
        GuardPhase::Authentication
    }
}

impl GuardFor<HttpView> for Authenticating {}

#[module(providers = [
    Authorizing,
    Authenticating,
    PoolTool,
    EndpointGuards<McpView, (Authorizing, Authenticating)>,
])]
struct MisorderedModule;

/// Endpoint guards authorizing before they authenticate fail the boot, as a
/// route's would, rather than refusing every request.
#[tokio::test]
async fn endpoint_guards_out_of_phase_order_fail_the_boot() {
    let Err(refused) = TestApp::for_module::<MisorderedModule>().await else {
        panic!("a misordered request stage would deny every caller");
    };
    let said = format!("{refused:#}");
    assert!(
        said.contains("Authorizing") && said.contains("Authenticating"),
        "{said}",
    );
}

/// What [`Attaching`] hands the operations of the request it checked.
struct Caller(&'static str);

impl AmbientValue for Caller {}

/// Attaches a [`Caller`] to every request, named its actor as the
/// authentication guard names it, so a session opens for it.
#[injectable]
#[derive(Default)]
struct Attaching;

impl Layer for Attaching {}

#[async_trait]
impl Guard for Attaching {
    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        nest_rs_core::__private::set_actor_id("ada");
        cx.attach(Caller("ada"));
        Ok(())
    }
}

impl GuardFor<HttpView> for Attaching {}

#[mcp(path = "/mcp/ambient")]
#[derive(Clone)]
struct AmbientProbeTool;

#[tool_router]
impl AmbientProbeTool {
    /// Answers over the wire, so no process-global flag is shared between tests.
    #[tool(description = "Report who the request's guards said the caller is.")]
    async fn probe_ambient(&self) -> Result<CallToolResult, McpError> {
        let caller = nest_rs_core::ambient::<Caller>().map_or("nobody", |caller| caller.0);
        Ok(CallToolResult::success(vec![ContentBlock::text(caller)]))
    }
}

#[tool_handler]
impl ServerHandler for AmbientProbeTool {}

#[module(providers = [
    Attaching,
    AmbientProbeTool,
    EndpointGuards<McpView, (Attaching,)>,
])]
struct AmbientModule;

/// What the request's guards attach is ambient in each operation, across
/// rmcp's own task.
#[tokio::test]
async fn what_the_endpoint_guards_attach_reaches_each_operation() {
    let app = TestApp::for_module::<AmbientModule>().await.expect("boots");

    let body = call_tool(app.http(), "/mcp/ambient", "probe_ambient", None).await;

    assert!(
        body.contains("ada") && !body.contains("nobody"),
        "the attachment must cross rmcp's spawn. Body: {body}",
    );
}

#[module(providers = [PoolTool, crate::StoreDown])]
struct StoreDownPoolModule;

/// A pooled guard that cannot evaluate the `/mcp` request files what failed on
/// the request's denial line, and the client reads the condition alone.
#[tokio::test]
async fn a_request_denial_keeping_its_cause_files_the_chain() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<StoreDownPoolModule>()
        .use_guards_global([guard::<crate::StoreDown>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().post("/mcp").send().await;
    resp.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    let body = resp
        .0
        .into_body()
        .into_string()
        .await
        .expect("a response body");
    assert!(
        !body.contains("refused"),
        "the cause stays on the server: {body}"
    );
    let event = logs.expect_one("nest_rs::layers", "guard denied the request");
    assert_eq!(
        event.field("error").as_deref(),
        Some(crate::STORE_DOWN_CHAIN)
    );
}
