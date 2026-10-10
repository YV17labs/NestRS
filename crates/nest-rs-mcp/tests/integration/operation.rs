//! `src/operation.rs` and the per-operation guard chain, run on the task rmcp
//! spawns, where no request is left.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard, McpGuard, async_trait};
use nest_rs_mcp::rmcp::serde_json::json;
use nest_rs_mcp::{
    AllowAllMcpGuard, McpError, McpOperationContext, McpOperationGuard, McpOperationKind, mcp,
    tools,
};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{call_method, call_tool, open_session};

const PATH: &str = "/mcp/layers";

/// Records what each check was told: a guard handed the wrong operation passes a
/// bare "did it run" assertion.
#[injectable]
#[derive(Default)]
struct Recorder {
    calls: AtomicUsize,
    tools: std::sync::Mutex<Vec<String>>,
}

impl Layer for Recorder {}

#[async_trait]
impl Guard for Recorder {
    async fn check_mcp(&self, ctx: &McpOperationContext<'_>) -> Result<(), Denial> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.tools
            .lock()
            .expect("the recorder's log is only touched here")
            .push(format!("{} {}", ctx.kind(), ctx.name()));
        Ok(())
    }
}

impl McpGuard for Recorder {}

/// Refuses everything, so a denial's shape can be read off the wire.
#[injectable]
#[derive(Default)]
struct Closed;

impl Layer for Closed {}

#[async_trait]
impl Guard for Closed {
    async fn check_mcp(&self, _ctx: &McpOperationContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("not for you"))
    }
}

impl McpGuard for Closed {}

#[mcp(path = "/mcp/layers")]
#[use_guards(Recorder)]
#[derive(Clone, Default)]
struct LayeredTool;

#[tools]
impl LayeredTool {
    /// Answer with a constant — the assertions are about what ran around it.
    #[tool]
    #[public]
    async fn open(&self) -> Result<String, McpError> {
        Ok("open".to_owned())
    }

    /// Gated by an operation-scope guard on top of the host's.
    #[tool]
    #[public]
    #[use_guards(Closed)]
    async fn shut(&self) -> Result<String, McpError> {
        Ok("never reached".to_owned())
    }

    /// A prompt is an operation like any other, so it takes the chain too.
    #[prompt]
    #[public]
    async fn draft(&self) -> Result<nest_rs_mcp::model::GetPromptResult, McpError> {
        Ok(nest_rs_mcp::model::GetPromptResult::new(vec![
            nest_rs_mcp::model::PromptMessage::new_text(nest_rs_mcp::model::Role::User, "hi"),
        ]))
    }
}

#[module(providers = [
    LayeredTool,
    Recorder,
    Closed,
    AllowAllMcpGuard as dyn McpOperationGuard,
])]
struct LayeredModule;

async fn boot() -> TestApp {
    TestApp::for_module::<LayeredModule>()
        .await
        .expect("a host declaring guards on itself and on an operation boots")
}

#[tokio::test]
async fn a_host_scope_guard_runs_for_every_operation() {
    let app = boot().await;
    let recorder = app
        .container()
        .get::<Recorder>()
        .expect("the guard is a provider");

    let body = call_tool(app.http(), PATH, "open", None).await;
    assert!(body.contains("open"), "the operation ran: {body}");

    let session = open_session(app.http(), PATH, None).await;
    call_method(
        app.http(),
        PATH,
        &session,
        None,
        "prompts/get",
        json!({ "name": "draft", "arguments": {} }),
    )
    .await;

    let seen = recorder
        .tools
        .lock()
        .expect("the recorder's log is only touched in the guard")
        .clone();
    assert_eq!(
        seen,
        ["tool open", "prompt draft"],
        "`#[use_guards]` on the host covers both roles, and each operation is \
         named to the guard that gates it",
    );
}

#[tokio::test]
async fn an_operation_scope_guard_refuses_that_operation_alone() {
    let app = boot().await;

    let refused = call_tool(app.http(), PATH, "shut", None).await;
    assert!(
        refused.contains("not for you"),
        "the operation-scope guard's denial reaches the client: {refused}",
    );
    assert!(
        refused.contains("forbidden"),
        "…carrying the machine-readable reason beside the message, so a client \
         can branch on it: {refused}",
    );
    assert!(
        !refused.contains("never reached"),
        "…and the body never ran: {refused}",
    );

    let allowed = call_tool(app.http(), PATH, "open", None).await;
    assert!(
        allowed.contains("open"),
        "…while its neighbour on the same host is untouched: {allowed}",
    );
}

#[tokio::test]
async fn a_guard_bound_to_an_operation_is_under_the_access_contract() {
    use nest_rs_core::Discoverable;

    let injected = LayeredTool::injected();
    assert!(
        injected.contains(&std::any::TypeId::of::<Recorder>()),
        "the host-scope guard is a declared dependency",
    );
    assert!(
        injected.contains(&std::any::TypeId::of::<Closed>()),
        "…and so is one bound beside a single operation — otherwise a module \
         nobody imported would surface as an ungated tool rather than a boot \
         error",
    );
}

#[tokio::test]
async fn the_operation_reports_its_kind_and_name() {
    let container = nest_rs_core::Container::default();
    let ctx = McpOperationContext::new(&container, "LayeredTool", McpOperationKind::Tool, "open");

    assert_eq!(ctx.host(), "LayeredTool");
    assert_eq!(ctx.kind(), McpOperationKind::Tool);
    assert_eq!(ctx.kind().as_str(), "tool");
    assert_eq!(ctx.name(), "open");
    assert_eq!(ctx.container().id(), container.id());
}

/// `static` counters are this test's alone: nextest runs each test in its own process.
static POOLED_HTTP_CALLS: AtomicUsize = AtomicUsize::new(0);
static POOLED_MCP_CALLS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct Pooled;

impl Layer for Pooled {}

#[async_trait]
impl Guard for Pooled {
    async fn check_http(&self, _req: &mut poem::Request) -> Result<(), Denial> {
        POOLED_HTTP_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn check_mcp(&self, _ctx: &McpOperationContext<'_>) -> Result<(), Denial> {
        POOLED_MCP_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl HttpGuard for Pooled {}
impl McpGuard for Pooled {}

#[mcp(path = "/mcp/pooled")]
#[use_guards(Pooled)]
#[derive(Clone, Default)]
struct PooledTool;

#[tools]
impl PooledTool {
    /// Answer with a constant.
    #[tool]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

#[module(providers = [PooledTool, Pooled])]
struct PooledModule;

#[tokio::test]
async fn a_pooled_guard_checks_the_request_once_and_the_operation_once() {
    let app = TestApp::builder()
        .use_guards_global([nest_rs_guards::guard::<Pooled>()])
        .module::<PooledModule>()
        .build()
        .await
        .expect("a pooled guard also declared on the host boots");

    // The handshake's own HTTP requests run the pool at the edge too.
    let session = open_session(app.http(), "/mcp/pooled", None).await;
    let http_before = POOLED_HTTP_CALLS.load(Ordering::SeqCst);
    let mcp_before = POOLED_MCP_CALLS.load(Ordering::SeqCst);

    let body = call_method(
        app.http(),
        "/mcp/pooled",
        &session,
        None,
        "tools/call",
        json!({ "name": "ping", "arguments": {} }),
    )
    .await;
    assert!(body.contains("pong"), "the operation ran: {body}");

    assert_eq!(
        POOLED_HTTP_CALLS.load(Ordering::SeqCst) - http_before,
        1,
        "the endpoint's `McpOperationGuard` folds the pool over the HTTP request \
         once — `exactly once per request` is what the whole Layer System promises",
    );
    assert_eq!(
        POOLED_MCP_CALLS.load(Ordering::SeqCst) - mcp_before,
        1,
        "and the operation is checked once too: the pool joins the per-operation \
         chain, where the host-scope declaration of the same guard dedups onto it \
         by `TypeId`. Charging the `check_http` against the `check_mcp` is what \
         used to leave this at zero",
    );
}

/// A global guard gating MCP alone: its presence disarms the deny-all tail, so it
/// must be consulted per operation or registering it opens the endpoint.
static POOL_ONLY_MCP_CALLS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct PoolOnlyMcp;

impl Layer for PoolOnlyMcp {}

#[async_trait]
impl Guard for PoolOnlyMcp {
    async fn check_mcp(&self, _ctx: &McpOperationContext<'_>) -> Result<(), Denial> {
        POOL_ONLY_MCP_CALLS.fetch_add(1, Ordering::SeqCst);
        Err(Denial::forbidden("the pool refuses this operation"))
    }
}

impl McpGuard for PoolOnlyMcp {}

#[mcp(path = "/mcp/unbridged")]
#[derive(Clone, Default)]
struct UnbridgedTool;

#[tools]
impl UnbridgedTool {
    /// Answer with a constant — reaching it at all is the failure.
    #[tool]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

#[module(providers = [UnbridgedTool, PoolOnlyMcp])]
struct UnbridgedModule;

#[tokio::test]
async fn a_global_guard_that_only_checks_mcp_still_refuses_the_operation() {
    let app = TestApp::builder()
        .use_guards_global([nest_rs_guards::guard::<PoolOnlyMcp>()])
        .module::<UnbridgedModule>()
        .build()
        .await
        .expect("a global MCP-only guard and no bridge boots");

    let body = call_tool(app.http(), "/mcp/unbridged", "ping", None).await;

    assert_eq!(
        POOL_ONLY_MCP_CALLS.load(Ordering::SeqCst),
        1,
        "the pool reaches the operation: the endpoint runs `check_http`, which \
         this guard does not implement, so the per-operation chain is the only \
         site that can consult it",
    );
    assert!(
        !body.contains("pong"),
        "the tool must not answer a caller its own global guard refused: {body}",
    );
    assert!(
        body.contains("the pool refuses this operation"),
        "and the denial reaches the client as the guard worded it: {body}",
    );
}

/// Global and declared on a `#[tool]`, under a bridge that runs nothing from the
/// pool: deduping onto the pool entry would leave the declaration never run.
static DECLARED_CALLS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct DeclaredEverywhere;

impl Layer for DeclaredEverywhere {}

#[async_trait]
impl Guard for DeclaredEverywhere {
    async fn check_mcp(&self, _ctx: &McpOperationContext<'_>) -> Result<(), Denial> {
        DECLARED_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl McpGuard for DeclaredEverywhere {}

/// Stands in for `McpAbilityBridge`, which runs nothing from the pool.
#[injectable]
#[derive(Default)]
struct BridgeStub;

impl McpOperationGuard for BridgeStub {
    fn before<'a>(
        &'a self,
        _req: &'a mut poem::Request,
    ) -> nest_rs_mcp::BoxFuture<'a, poem::Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

#[mcp(path = "/mcp/bridged")]
#[derive(Clone, Default)]
struct BridgedTool;

#[tools]
impl BridgedTool {
    /// Answer with a constant.
    #[tool]
    #[public]
    #[use_guards(DeclaredEverywhere)]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

#[module(providers = [
    BridgedTool,
    DeclaredEverywhere,
    BridgeStub as dyn McpOperationGuard,
])]
struct BridgedModule;

#[tokio::test]
async fn a_guard_the_edge_did_not_run_still_runs_per_operation() {
    let app = TestApp::builder()
        .use_guards_global([nest_rs_guards::guard::<DeclaredEverywhere>()])
        .module::<BridgedModule>()
        .build()
        .await
        .expect("a bridge plus a guard declared both globally and on the tool boots");

    let body = call_tool(app.http(), "/mcp/bridged", "ping", None).await;
    assert!(body.contains("pong"), "the operation ran: {body}");
    assert_eq!(
        DECLARED_CALLS.load(Ordering::SeqCst),
        1,
        "the registered bridge runs its own guards and nothing from the pool, so \
         the guard declared beside this tool must still execute — dropping it \
         because its `TypeId` also appears in the pool is a fail-open on a \
         declaration the developer wrote",
    );
}

/// Listed by a host's `#[use_guards]`, provided by no module.
#[injectable]
#[derive(Default)]
struct NeverProvided;

impl Layer for NeverProvided {}

#[async_trait]
impl Guard for NeverProvided {
    async fn check_mcp(&self, _ctx: &McpOperationContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl McpGuard for NeverProvided {}

#[mcp(path = "/mcp/orphan")]
#[use_guards(NeverProvided)]
#[derive(Clone, Default)]
struct OrphanTool;

#[tools]
impl OrphanTool {
    /// Answer with a constant the client must never read.
    #[tool]
    #[public]
    async fn reveal(&self) -> Result<String, McpError> {
        Ok("classified".to_owned())
    }
}

#[tokio::test]
async fn an_operation_whose_guard_no_module_provides_is_refused_opaquely_and_says_why() {
    use std::sync::Arc;

    use poem::{Endpoint, EndpointExt, IntoEndpoint};

    // Built by hand, so the access graph does not refuse first; the request
    // scope stands in for the edge that installs it.
    let container = nest_rs_core::Container::builder().build();
    let guard = Arc::new(AllowAllMcpGuard) as Arc<dyn McpOperationGuard>;
    let inner = Arc::new(
        nest_rs_mcp::endpoint(nest_rs_mcp::McpMount::deny_all().with_guard(guard), || {
            OrphanTool
        })
        .into_endpoint()
        .map_to_response(),
    );
    let scoped = poem::endpoint::make(move |req| {
        let inner = Arc::clone(&inner);
        let scope = Arc::new(nest_rs_core::RequestScope::new(container.clone()));
        let correlation = nest_rs_core::Correlation::minted(None);
        async move { nest_rs_core::with_request_scope(Some(scope), correlation, inner.call(req)).await }
    });
    let client = poem::test::TestClient::new(scoped);
    let logs = nest_rs_testing::LogCapture::install();

    let body = call_tool(&client, "/", "reveal", None).await;

    assert!(
        !body.contains("classified"),
        "the operation never ran without its guard: {body}",
    );
    assert!(
        !body.contains("NeverProvided"),
        "the client never reads the wiring: {body}",
    );
    let event = logs.expect_one(
        "nest_rs::layers",
        "a layer the site declares is provided by no imported module",
    );
    assert_eq!(event.level, "error");
    assert!(
        event
            .field("layer")
            .is_some_and(|layer| layer.contains("NeverProvided")),
        "the line names the layer: {:?}",
        event.fields,
    );
}
