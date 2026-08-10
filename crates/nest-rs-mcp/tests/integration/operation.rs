//! The ambient MCP operation — `src/operation.rs`, and the per-operation guard
//! chain that hangs off it.
//!
//! An operation is dispatched on a task rmcp spawned, with no request and no
//! context parameter to carry the app through. These assert that the seam
//! closing that gap actually closes it: a `#[use_guards]` beside a `#[tool]`
//! runs, a host-scope one runs for every operation, and both reach the caller's
//! ambient state rather than a request that no longer exists.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, McpGuard, async_trait};
use nest_rs_mcp::rmcp::serde_json::json;
use nest_rs_mcp::{
    AllowAllMcpGuard, McpError, McpOperationContext, McpOperationGuard, McpOperationKind, mcp,
    tools,
};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{call_method, call_tool, open_session};

const PATH: &str = "/mcp/layers";

/// Counts what it saw, so a test can assert a guard ran *and* what it was told
/// about the operation — a guard that runs but is handed the wrong operation is
/// the failure a bare "did it run" assertion misses.
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
    // `McpOperationContext` is what a guard is handed; the accessors are the
    // whole surface, so they are asserted rather than assumed.
    let container = nest_rs_core::Container::default();
    let ctx = McpOperationContext::new(&container, "LayeredTool", McpOperationKind::Tool, "open");

    assert_eq!(ctx.host(), "LayeredTool");
    assert_eq!(ctx.kind(), McpOperationKind::Tool);
    assert_eq!(ctx.kind().as_str(), "tool");
    assert_eq!(ctx.name(), "open");
    assert_eq!(ctx.container().id(), container.id());
}

/// A guard in the app-wide pool that also counts. `static` rather than an
/// injected handle because nextest runs each test in its own process, so the
/// counter is this test's alone.
static POOLED_CALLS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct Pooled;

impl Layer for Pooled {}

#[async_trait]
impl Guard for Pooled {
    async fn check_http(&self, _req: &mut poem::Request) -> Result<(), Denial> {
        POOLED_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn check_mcp(&self, _ctx: &McpOperationContext<'_>) -> Result<(), Denial> {
        POOLED_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

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

/// The endpoint's guard reports what it ran; an operation's own chain drops
/// exactly that and nothing more.
#[tokio::test]
async fn a_pooled_guard_is_not_charged_twice_per_operation() {
    let app = TestApp::builder()
        .use_guards_global([nest_rs_guards::guard::<Pooled>()])
        .module::<PooledModule>()
        .build()
        .await
        .expect("a pooled guard also declared on the host boots");

    // Measure the tool call alone: the handshake before it is two more HTTP
    // requests, and each of those legitimately runs the pool at the edge once.
    let session = open_session(app.http(), "/mcp/pooled", None).await;
    let before = POOLED_CALLS.load(Ordering::SeqCst);

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
        POOLED_CALLS.load(Ordering::SeqCst) - before,
        1,
        "one run for the request, and none for the operation: the pool already \
         ran at the endpoint's `McpOperationGuard`, so the host-scope duplicate \
         dedups onto it instead of running a second time — `exactly once per \
         request` is what the whole Layer System promises",
    );
}

/// The other half of the same rule, and the one a naive dedup gets wrong: a
/// guard the endpoint did **not** run must still run per operation, even when
/// the app-wide pool happens to contain it.
///
/// The failing shape is an app with a registered bridge — the documented norm —
/// plus a guard that is both global and declared on a `#[tool]`. Deduping the
/// scoped declaration onto the pool entry and then dropping the pool entry
/// leaves nothing, so a `#[use_guards]` the developer wrote silently never runs.
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

/// Stands in for `McpAbilityBridge`: gates the request itself and reports that
/// it ran nothing from the pool — which is the truth for a real bridge.
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
