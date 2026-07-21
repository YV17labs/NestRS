//! `src/guard.rs` + the guard half of `src/endpoint.rs`: the mount's guard
//! preference order (registered bridge → global pool → deny-all) and the
//! `around` seam that installs an operation's ambient state inside rmcp's
//! spawned dispatch.

use std::sync::Arc;

use nest_rs_core::module;
use nest_rs_guards::{Denial, Guard, guard};
use nest_rs_http::async_trait;
use nest_rs_mcp::{
    AllowAllMcpGuard, BoxFuture, CallToolResult, Captured, ContentBlock, McpError,
    McpOperationGuard, OperationOutcome, ServerHandler, endpoint_with_guard, mcp, tool,
    tool_handler, tool_router,
};
use nest_rs_testing::{TestApp, mcp::call_tool};
use nest_rs_throttler::{ThrottlerConfig, ThrottlerGuard, ThrottlerModule};
use poem::http::StatusCode;
use poem::test::TestClient;
use poem::{Error, Request, Response};

struct RejectGuard;

impl McpOperationGuard for RejectGuard {
    fn before<'a>(&'a self, _req: &'a mut Request) -> BoxFuture<'a, poem::Result<()>> {
        Box::pin(async move {
            Err(Error::from_response(
                Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .body("nope"),
            ))
        })
    }
}

#[derive(Clone)]
struct DummyHandler;

#[tool_router]
impl DummyHandler {}

#[tool_handler]
impl ServerHandler for DummyHandler {}

#[tokio::test]
async fn endpoint_with_guard_rejects_before_the_handler_runs() {
    let guarded = endpoint_with_guard(
        Arc::new(RejectGuard) as Arc<dyn McpOperationGuard>,
        None,
        || DummyHandler,
    );
    let resp = TestClient::new(guarded).post("/").send().await;
    assert_eq!(resp.0.status(), StatusCode::UNAUTHORIZED);
}

// `endpoint` picks the deny-all guard, and `resolve_operation_guard`'s tail
// picks the same one — so an app with neither a registered bridge nor a global
// pool fails closed rather than serving the tool surface open.
#[tokio::test]
async fn endpoint_without_an_explicit_guard_is_denied_by_default() {
    let open = nest_rs_mcp::endpoint(|| DummyHandler);
    let resp = TestClient::new(open).post("/").send().await;
    assert_eq!(resp.0.status(), StatusCode::UNAUTHORIZED);
}

// The explicit opt-in counterpart to deny-all: wiring `AllowAllMcpGuard`
// admits the request so a deliberately public tool can be served.
#[tokio::test]
async fn allow_all_guard_admits_the_request() {
    let guard = nest_rs_mcp::AllowAllMcpGuard;
    let mut req = Request::default();
    assert!(guard.before(&mut req).await.is_ok());
}

// --- `around`: ambient state crosses rmcp's spawn --------------------------

tokio::task_local! {
    /// Stand-in for the authz bridge's ambient `Ability`.
    static AMBIENT: String;
}

/// What `before` attaches to the request and `around` must find again from
/// inside the spawned dispatch.
#[derive(Clone)]
struct Attached(String);

/// The shape `McpAbilityBridge` has: `before` attaches per-request state,
/// `around` installs it as a task-local for the operation's duration.
struct AmbientGuard;

impl McpOperationGuard for AmbientGuard {
    fn before<'a>(&'a self, req: &'a mut Request) -> BoxFuture<'a, poem::Result<()>> {
        Box::pin(async move {
            req.extensions_mut()
                .insert(Attached("installed".to_owned()));
            Ok(())
        })
    }

    fn capture(&self, req: &Request) -> Option<Captured> {
        req.extensions()
            .get::<Attached>()
            .cloned()
            .map(|attached| Arc::new(attached) as Captured)
    }

    fn around<'a>(
        &'a self,
        captured: &'a Captured,
        inner: BoxFuture<'a, OperationOutcome>,
    ) -> BoxFuture<'a, OperationOutcome> {
        Box::pin(async move {
            match captured.downcast_ref::<Attached>() {
                Some(attached) => AMBIENT.scope(attached.0.clone(), inner).await,
                None => inner.await,
            }
        })
    }
}

#[derive(Clone)]
struct AmbientProbeTool;

#[tool_router]
impl AmbientProbeTool {
    /// Reports over the wire whether the guard's ambient state reached the
    /// body — the response *is* the assertion, so there is no process-global
    /// flag for a second test to clobber.
    #[tool(description = "Report whether the guard's ambient state reached this tool body.")]
    async fn probe_ambient(&self) -> Result<CallToolResult, McpError> {
        let seen = AMBIENT.try_with(|value| value.clone()).is_ok();
        Ok(CallToolResult::success(vec![ContentBlock::text(if seen {
            "ambient"
        } else {
            "bare"
        })]))
    }
}

#[tool_handler]
impl ServerHandler for AmbientProbeTool {}

// D2: the guard — not the data context — installs the operation's ambient
// state, and it lands *inside* rmcp's spawned dispatch. No `McpToolContext` is
// registered here, which is the point: an app that binds the guard but forgets
// the data context still gets a scoped tool body.
#[tokio::test]
async fn an_operation_guards_around_installs_ambient_state_with_no_tool_context() {
    let guard = Arc::new(AmbientGuard) as Arc<dyn McpOperationGuard>;
    let client = TestClient::new(endpoint_with_guard(guard, None, || AmbientProbeTool));

    let body = call_tool(&client, "/", "probe_ambient", None).await;

    assert!(
        body.contains("ambient") && !body.contains("bare"),
        "the guard's `around` must install its state across rmcp's spawn — \
         without it the ambient ability only exists when `McpDataContext` is \
         also registered. Body: {body}",
    );
}

// --- the global-pool fallback ---------------------------------------------

#[mcp(path = "/mcp")]
#[derive(Clone)]
struct PoolTool;

#[tool_router]
impl PoolTool {}

#[tool_handler]
impl ServerHandler for PoolTool {}

#[module(imports = [ThrottlerModule::for_root(one_per_minute())], providers = [PoolTool, ThrottlerGuard])]
struct PoolModule;

fn one_per_minute() -> ThrottlerConfig {
    ThrottlerConfig {
        limit: Some(1),
        window_secs: Some(60),
        trusted_proxies: Vec::new(),
    }
}

/// A guard that refuses everything, to prove which side of the preference
/// order actually ran.
#[nest_rs_core::injectable]
#[derive(Default)]
struct DenyEverythingGuard;

impl nest_rs_core::Layer for DenyEverythingGuard {}

#[async_trait]
impl Guard for DenyEverythingGuard {
    async fn check_http(&self, _req: &mut Request) -> Result<(), Denial> {
        Err(Denial::forbidden("global pool refused"))
    }
}

#[module(providers = [PoolTool, DenyEverythingGuard, AllowAllMcpGuard as dyn McpOperationGuard])]
struct BridgeWinsModule;

#[module(providers = [PoolTool])]
struct NoPoolModule;

// D1, the headline: `use_guards_global` reaches `/mcp` the way it already
// reaches `/graphql`. Before the fallback existed a global `ThrottlerGuard`
// could not rate-limit a tool call at all — `/mcp` was either dead (401) or
// ungoverned by the pool.
#[tokio::test]
async fn a_global_throttler_guard_rate_limits_mcp() {
    let app = TestApp::builder()
        .module::<PoolModule>()
        .use_guards_global([guard::<ThrottlerGuard>()])
        .build()
        .await
        .expect("boots with a global throttler and no MCP bridge");

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

// A registered `dyn McpOperationGuard` **replaces** the fallback — it owns the
// chain, exactly as a registered `GraphqlOperationGuard` bridge does.
#[tokio::test]
async fn a_registered_guard_replaces_the_global_pool_fallback() {
    let app = TestApp::builder()
        .module::<BridgeWinsModule>()
        .use_guards_global([guard::<DenyEverythingGuard>()])
        .build()
        .await
        .expect("boots");

    let resp = app.http().post("/mcp").send().await;
    assert_ne!(
        resp.0.status(),
        StatusCode::FORBIDDEN,
        "the registered guard owns the chain; the global pool must not also run",
    );
}

// The fallback only ever *widens* what the app opted into: with no global pool
// at all, `/mcp` stays deny-all rather than becoming an empty-chain pass.
#[tokio::test]
async fn no_global_pool_leaves_mcp_deny_all() {
    let app = TestApp::for_module::<NoPoolModule>().await.expect("boots");
    app.http()
        .post("/mcp")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
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
