//! The guards at the `/mcp` request — the global pool and the
//! `EndpointGuards<McpView, _>` the app's modules declare, run by the transport
//! in front of the endpoint — and what they refuse, as `src/guard.rs` renders
//! it.

use std::sync::{Arc, Mutex};

use nest_rs_core::{AmbientValue, Layer, UnitContext, async_trait, injectable, module};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor, GuardPhase};
use nest_rs_http::{EndpointGuards, HttpView, StatusCode};
use nest_rs_mcp::model::{CallToolResult, ContentBlock};
use nest_rs_mcp::rmcp::{tool, tool_handler, tool_router};
use nest_rs_mcp::{McpAccess, McpError, McpModule, McpView, ServerHandler, mcp, tools};
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
#[module(
    imports = [ThrottlerModule::for_root(one_per_minute())],
    providers = [Attaching, PoolTool],
)]
struct PoolModule;

fn one_per_minute() -> ThrottlerConfig {
    ThrottlerConfig {
        limit: 1,
        window: std::time::Duration::from_secs(60),
        pseudonym_key: None,
    }
}

#[module(imports = [PoolModule], providers = [GlobalGuards<(Attaching, ThrottlerGuard)>])]
struct AGlobalThrottlerGuardRateLimitsMcpApp;

#[tokio::test]
async fn a_global_throttler_guard_rate_limits_mcp() {
    let app = TestApp::builder()
        .module::<AGlobalThrottlerGuardRateLimitsMcpApp>()
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

    let announced = logs.expect_one("nest_rs::layers", "no guard at the request");
    assert_eq!(announced.level, "warn");
    assert_eq!(announced.field("surface").as_deref(), Some("mcp"));
    assert_eq!(announced.field("path").as_deref(), Some("/mcp"));
    assert_eq!(
        announced.field("then").as_deref(),
        Some("denies every unit")
    );
    assert_eq!(
        announced.field("hint").as_deref(),
        Some(nest_rs_http::__private::NO_GUARD_REMEDY)
    );
    let denied = logs.expect_one(
        "nest_rs::layers",
        "request denied: the endpoint runs no guard at its request",
    );
    assert_eq!(denied.field("path").as_deref(), Some("/mcp"));
}

/// A guard an `#[mcp]` host binds runs at its operations, which no request
/// reaches through a stage that denies: the endpoint stays closed, and the
/// line's hint names what lifts the stage.
mod a_hosts_own_guards {
    use nest_rs_mcp::tools;

    use super::*;

    #[injectable]
    #[derive(Default, Layer)]
    struct Pass;

    #[async_trait]
    impl Guard for Pass {
        fn phase(&self) -> nest_rs_guards::GuardPhase {
            nest_rs_guards::GuardPhase::Authorization
        }

        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
    }

    impl GuardFor<McpView> for Pass {}

    #[mcp]
    #[derive(Default)]
    #[use_guards(Pass)]
    struct ScopedTool;

    #[tools]
    impl ScopedTool {
        #[tool(description = "ping")]
        async fn ping(&self) -> Result<String, McpError> {
            Ok("pong".into())
        }
    }

    #[module(providers = [Pass, ScopedTool])]
    struct ScopedToolModule;

    #[tokio::test]
    async fn lift_nothing_at_the_request_and_the_hint_names_what_does() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<ScopedToolModule>()
            .await
            .expect("boots");

        post_message(app.http(), "/mcp", None, None, &initialize_request())
            .await
            .assert_status(StatusCode::UNAUTHORIZED);

        let line = logs.expect_one("nest_rs::layers", "no guard at the request");
        assert_eq!(line.field("then").as_deref(), Some("denies every unit"));
        let hint = line.field("hint").unwrap_or_default();
        assert!(
            hint.contains("`EndpointGuards<View, (...)>` on an endpoint carrying units"),
            "{hint}",
        );
        assert!(
            !hint.contains("on the host"),
            "a host's guards lift no request stage: {hint}",
        );
    }
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
        #[derive(Layer)]
        struct $name {
            #[inject]
            trail: Arc<Trail>,
        }

        #[async_trait]
        impl Guard for $name {
            fn phase(&self) -> nest_rs_guards::GuardPhase {
                nest_rs_guards::GuardPhase::Authentication
            }

            async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
                self.trail.seen.lock().expect("trail lock").push($label);
                nest_rs_core::__private::set_actor_id($label);
                Ok(())
            }

            async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
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

#[module(imports = [DeclaredModule], providers = [GlobalGuards<(Pooled,)>])]
struct EndpointGuardsComposeWithThePoolApp;

/// The pool and the endpoint guards both run at the request, each once: a guard
/// in both runs where the pool lists it.
#[tokio::test]
async fn endpoint_guards_compose_with_the_pool() {
    let app = TestApp::builder()
        .module::<EndpointGuardsComposeWithThePoolApp>()
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
    imports = [McpModule::for_root(McpAccess::Public)],
    providers = [PoolTool, DenyEverything],
)]
struct OpenedModule;

/// Refuses every request.
#[injectable]
#[derive(Default, Layer)]
struct DenyEverything;

#[async_trait]
impl Guard for DenyEverything {
    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("the pool refused"))
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for DenyEverything {}

#[module(imports = [OpenedModule], providers = [GlobalGuards<(DenyEverything,)>])]
struct McpAccessPublicOpensTheEndpointApp;

/// The app opens `/mcp` in code: no guard runs at its request, the global
/// pool's included.
#[tokio::test]
async fn mcp_access_public_opens_the_endpoint() {
    let app = TestApp::builder()
        .module::<McpAccessPublicOpensTheEndpointApp>()
        .build()
        .await
        .expect("boots");

    post_message(app.http(), "/mcp", None, None, &initialize_request())
        .await
        .assert_status(StatusCode::OK);
}

#[module(
    imports = [OpenedModule, ThrottlerModule::for_root(one_per_minute())],
    providers = [GlobalGuards<(ThrottlerGuard,)>],
)]
struct OpenedUnderAThrottlerApp;

/// The pool runs at no request of an opened endpoint, so a throttler in it
/// meters none: past its budget, a session still opens.
#[tokio::test]
async fn a_throttler_in_the_pool_meters_no_request_of_an_opened_endpoint() {
    let app = TestApp::builder()
        .module::<OpenedUnderAThrottlerApp>()
        .build()
        .await
        .expect("boots");

    for _ in 0..3 {
        post_message(app.http(), "/mcp", None, None, &initialize_request())
            .await
            .assert_status(StatusCode::OK);
    }
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
        contested
            .remedy
            .contains("pass `McpAccess::Public` to one `McpModule::for_root`"),
        "the remedy says which declaration to make once: {}",
        contested.remedy,
    );
}

#[module(
    imports = [McpModule::for_root(McpAccess::Public)],
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
#[derive(Default, Layer)]
struct Authorizing;

#[async_trait]
impl Guard for Authorizing {
    async fn check(&self, _cx: &mut UnitContext<'_>) -> std::result::Result<(), Denial> {
        Ok(())
    }

    fn phase(&self) -> GuardPhase {
        GuardPhase::Authorization
    }
}

impl GuardFor<HttpView> for Authorizing {}
impl GuardFor<McpView> for Authorizing {}

/// Authenticates, declaring it.
#[injectable]
#[derive(Default, Layer)]
struct Authenticating;

#[async_trait]
impl Guard for Authenticating {
    async fn check(&self, _cx: &mut UnitContext<'_>) -> std::result::Result<(), Denial> {
        Ok(())
    }

    fn phase(&self) -> GuardPhase {
        GuardPhase::Authentication
    }
}

impl GuardFor<HttpView> for Authenticating {}
impl GuardFor<McpView> for Authenticating {}

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

/// `boot` refused for an authentication guard after an authorization guard, naming the two.
fn assert_misordered(boot: nest_rs_core::anyhow::Result<TestApp>) {
    let Err(refused) = boot else {
        panic!("the misordered chain booted");
    };
    let refused = format!("{refused:#}");
    assert!(
        refused.contains("Authorizing") && refused.contains("Authenticating"),
        "{refused}",
    );
    assert!(refused.contains("after authorization guard"), "{refused}");
}

#[mcp(path = "/mcp/misordered")]
#[use_guards(Authorizing, Authenticating)]
#[derive(Clone)]
struct MisorderedTool;

#[tools]
impl MisorderedTool {
    /// Read.
    #[tool]
    async fn read_misordered(&self) -> Result<String, McpError> {
        Ok(String::new())
    }
}

#[module(providers = [Authorizing, Authenticating, MisorderedTool])]
struct MisorderedToolModule;

#[tokio::test]
async fn a_tool_listing_authentication_after_authorization_refuses_the_boot() {
    assert_misordered(TestApp::for_module::<MisorderedToolModule>().await);
}

#[mcp(path = "/mcp/after-its-request")]
#[use_guards(Authenticating)]
#[derive(Clone)]
struct AfterItsRequestTool;

#[tools]
impl AfterItsRequestTool {
    /// Read.
    #[tool]
    async fn read_after_its_request(&self) -> Result<String, McpError> {
        Ok(String::new())
    }
}

#[module(providers = [
    Authorizing,
    Authenticating,
    AfterItsRequestTool,
    EndpointGuards<McpView, (Authorizing,)>,
])]
struct AfterItsRequestToolModule;

#[tokio::test]
async fn a_tool_authenticating_after_its_requests_authorization_refuses_the_boot() {
    assert_misordered(TestApp::for_module::<AfterItsRequestToolModule>().await);
}

/// What [`Attaching`] hands the operations of the request it checked.
struct Caller(&'static str);

impl AmbientValue for Caller {}

/// Attaches a [`Caller`] to every request, named its actor as the
/// authentication guard names it, so a session opens for it.
#[injectable]
#[derive(Default, Layer)]
struct Attaching;

#[async_trait]
impl Guard for Attaching {
    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        nest_rs_core::__private::set_actor_id("ada");
        cx.attach(Caller("ada"));
        Ok(())
    }

    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
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

#[module(imports = [StoreDownPoolModule], providers = [GlobalGuards<(crate::StoreDown,)>])]
struct ARequestDenialKeepingItsCauseFilesTheChainApp;

/// A pooled guard that cannot evaluate the `/mcp` request files what failed on
/// the request's denial line, and the client reads the condition alone.
#[tokio::test]
async fn a_request_denial_keeping_its_cause_files_the_chain() {
    let logs = LogCapture::install();
    let app = TestApp::builder()
        .module::<ARequestDenialKeepingItsCauseFilesTheChainApp>()
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

/// A pooled guard deciding on the HTTP request: in `check_http`, its `check`
/// admitting, it gates the `/mcp` request and every operation it carries
/// answers; in `check`, it runs on each operation too, which has no HTTP view,
/// and the line says which guard and what to move.
mod request_decided {
    use nest_rs_core::{Layer, UnitContext, async_trait, injectable, module};
    use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor};
    use nest_rs_http::{HttpView, StatusCode};
    use nest_rs_mcp::{McpError, McpView, mcp, tools};
    use nest_rs_testing::mcp::{call_tool_statelessly, post_message};
    use nest_rs_testing::{LogCapture, TestApp};
    use serde_json::json;

    fn keyed(cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        match HttpView::of(cx)?.request().header("x-api-key") {
            Some(_) => Ok(()),
            None => Err(Denial::unauthorized("missing key")),
        }
    }

    #[injectable]
    #[derive(Default, Layer)]
    struct KeyAtTheRequest;

    #[async_trait]
    impl Guard for KeyAtTheRequest {
        fn phase(&self) -> nest_rs_guards::GuardPhase {
            nest_rs_guards::GuardPhase::Authorization
        }

        async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            keyed(cx)
        }

        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
    }

    impl GuardFor<HttpView> for KeyAtTheRequest {}
    impl GuardFor<McpView> for KeyAtTheRequest {}

    #[injectable]
    #[derive(Default, Layer)]
    struct KeyAtTheUnit;

    #[async_trait]
    impl Guard for KeyAtTheUnit {
        fn phase(&self) -> nest_rs_guards::GuardPhase {
            nest_rs_guards::GuardPhase::Authorization
        }

        async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            keyed(cx)
        }

        async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            keyed(cx)
        }
    }

    impl GuardFor<HttpView> for KeyAtTheUnit {}
    impl GuardFor<McpView> for KeyAtTheUnit {}

    #[mcp(path = "/mcp/keyed")]
    #[derive(Clone, Default)]
    struct KeyedTool;

    #[tools]
    impl KeyedTool {
        /// Answer with a constant.
        #[tool]
        async fn ping(&self) -> Result<String, McpError> {
            Ok("pong".to_owned())
        }
    }

    #[module(providers = [KeyAtTheRequest, KeyAtTheUnit, KeyedTool])]
    struct KeyModule;

    #[module(imports = [KeyModule], providers = [GlobalGuards<(KeyAtTheRequest,)>])]
    struct RequestKeyPoolModule;

    #[module(imports = [KeyModule], providers = [GlobalGuards<(KeyAtTheUnit,)>])]
    struct UnitKeyPoolModule;

    const KEY: &[(&str, &str)] = &[("x-api-key", "k")];

    #[tokio::test]
    async fn a_pooled_guard_deciding_in_check_http_gates_the_request_and_admits_its_operations() {
        let app = TestApp::for_module::<RequestKeyPoolModule>()
            .await
            .expect("boots");

        let body =
            call_tool_statelessly(app.http(), "/mcp/keyed", "ping", None, KEY, json!({})).await;
        assert!(body.contains("pong"), "the operation ran: {body}");

        let refused = post_message(
            app.http(),
            "/mcp/keyed",
            None,
            None,
            &nest_rs_testing::mcp::initialize_request(),
        )
        .await;
        assert_eq!(refused.0.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_pooled_guard_reading_the_request_in_check_refuses_each_operation_naming_itself() {
        let logs = LogCapture::install();
        let app = TestApp::for_module::<UnitKeyPoolModule>()
            .await
            .expect("boots");

        for _ in 0..2 {
            let body =
                call_tool_statelessly(app.http(), "/mcp/keyed", "ping", None, KEY, json!({})).await;
            assert!(!body.contains("pong"), "the operation is refused: {body}");
        }

        let denials = logs.find("nest_rs::layers", "guard denied the operation");
        assert_eq!(denials.len(), 2, "each refused operation is filed once");
        for denial in denials {
            assert_eq!(denial.level, "error");
            assert_eq!(denial.field("guard").as_deref(), Some("KeyAtTheUnit"));
            assert!(
                denial
                    .field("error")
                    .is_some_and(|chain| chain.contains("decide the request in `check_http`")),
                "the edge's line says the edit: {denial:?}"
            );
        }
        logs.expect_none(
            "nest_rs::http",
            "a guard read the HTTP request of a unit with no HTTP view",
        );
    }
}
