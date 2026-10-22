//! `src/operation.rs` and the per-operation guard chain, run on the task rmcp
//! spawns, where no request is left.

use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs_mcp::rmcp::serde_json::json;
use nest_rs_mcp::{McpError, McpOperationKind, mcp, tools};
use nest_rs_testing::TestApp;
use nest_rs_testing::mcp::{call_method, call_tool, call_tool_statelessly, open_session};

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
    async fn check(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let view = cx
            .switch_to::<nest_rs_mcp::McpView>()
            .ok_or_else(|| Denial::internal("an MCP operation has the MCP view"))?;
        let kind = view
            .kind()
            .ok_or_else(|| Denial::internal("a tool call or a prompt fetch names its router"))?;
        let seen = format!("{kind} {}", view.name().unwrap_or_default());
        assert_eq!(
            view.method(),
            match kind {
                McpOperationKind::Tool => "tools/call",
                McpOperationKind::Prompt => "prompts/get",
            },
            "the view names the JSON-RPC method its router answers",
        );
        assert!(
            view.host()
                .is_some_and(|host| host.ends_with("::LayeredTool")),
            "the view names its host type"
        );
        assert_eq!(
            cx.handler().address(),
            view.name().unwrap_or_default(),
            "the site the unit runs as is the operation's own",
        );
        self.tools
            .lock()
            .expect("the recorder's log is only touched here")
            .push(seen);
        Ok(())
    }
}

impl GuardFor<nest_rs_mcp::McpView> for Recorder {}

/// Refuses everything, so a denial's shape can be read off the wire.
#[injectable]
#[derive(Default)]
struct Closed;

impl Layer for Closed {}

#[async_trait]
impl Guard for Closed {
    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Err(Denial::forbidden("not for you"))
    }
}

impl GuardFor<nest_rs_mcp::McpView> for Closed {}

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

    /// Gated by a guard whose identity store is down.
    #[tool]
    #[public]
    #[use_guards(crate::StoreDown)]
    async fn down(&self) -> Result<String, McpError> {
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

#[module(imports = [crate::open_mcp()], providers = [
    LayeredTool,
    Recorder,
    Closed,
    crate::StoreDown])]
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

/// A guard that cannot evaluate the operation files what failed on the
/// operation's denial line, and the client reads the condition alone.
#[tokio::test]
async fn an_operation_denial_keeping_its_cause_files_the_chain() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = boot().await;

    let refused = call_tool(app.http(), PATH, "down", None).await;
    assert!(
        refused.contains(nest_rs_core::UNAVAILABLE_CLIENT_MESSAGE),
        "the client reads the condition: {refused}",
    );
    assert!(
        !refused.contains("refused the connection") && !refused.contains("never reached"),
        "…never the cause, and the body never ran: {refused}",
    );
    let event = logs.expect_one("nest_rs::layers", "guard denied the operation");
    assert_eq!(event.field("operation").as_deref(), Some("down"));
    assert_eq!(
        event.field("error").as_deref(),
        Some(crate::STORE_DOWN_CHAIN)
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

#[test]
fn an_operation_kind_reads_as_the_router_serving_it() {
    assert_eq!(McpOperationKind::Tool.as_str(), "tool");
    assert_eq!(McpOperationKind::Prompt.to_string(), "prompt");
}

/// `static` counters are this test's alone: nextest runs each test in its own process.
static POOLED_HTTP_CALLS: AtomicUsize = AtomicUsize::new(0);
static POOLED_MCP_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Admits every request, naming its caller as the authentication guard does,
/// and every operation.
#[injectable]
#[derive(Default)]
struct Pooled;

impl Layer for Pooled {}

#[async_trait]
impl Guard for Pooled {
    async fn check_http(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        POOLED_HTTP_CALLS.fetch_add(1, Ordering::SeqCst);
        nest_rs_core::__private::set_actor_id("pooled");
        Ok(())
    }

    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        POOLED_MCP_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for Pooled {}
impl GuardFor<nest_rs_mcp::McpView> for Pooled {}

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
        "the transport folds the pool over the HTTP request once — `exactly \
         once per request` is what the whole Layer System promises",
    );
    assert_eq!(
        POOLED_MCP_CALLS.load(Ordering::SeqCst) - mcp_before,
        1,
        "and the operation is checked once too: the pool joins the per-operation \
         chain, where the host-scope declaration of the same guard dedups onto it \
         by `TypeId`. Charging the request's `check_http` against the \
         operation's `check` is what used to leave this at zero",
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
    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        POOL_ONLY_MCP_CALLS.fetch_add(1, Ordering::SeqCst);
        Err(Denial::forbidden("the pool refuses this operation"))
    }
}

impl GuardFor<nest_rs_mcp::McpView> for PoolOnlyMcp {}

#[mcp(path = "/mcp/pool-only")]
#[derive(Clone, Default)]
struct PoolOnlyTool;

#[tools]
impl PoolOnlyTool {
    /// Answer with a constant — reaching it at all is the failure.
    #[tool]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

#[module(providers = [PoolOnlyTool, PoolOnlyMcp])]
struct PoolOnlyModule;

#[tokio::test]
async fn a_global_guard_that_only_checks_mcp_still_refuses_the_operation() {
    let app = TestApp::builder()
        .use_guards_global([nest_rs_guards::guard::<PoolOnlyMcp>()])
        .module::<PoolOnlyModule>()
        .build()
        .await
        .expect("a global MCP-only guard and no endpoint guards boots");

    // No guard at the request names a caller, so no session opens for one:
    // a stateless call reaches the operation.
    let body =
        call_tool_statelessly(app.http(), "/mcp/pool-only", "ping", None, &[], json!({})).await;

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

/// Global and declared on a `#[tool]`, under endpoint guards that run nothing
/// from the pool: deduping onto the pool entry would leave the declaration never
/// run.
static DECLARED_CALLS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct DeclaredEverywhere;

impl Layer for DeclaredEverywhere {}

#[async_trait]
impl Guard for DeclaredEverywhere {
    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        DECLARED_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl GuardFor<nest_rs_mcp::McpView> for DeclaredEverywhere {}

/// Admits every request, naming its caller as the authentication guard does:
/// the endpoint's own guard, beside the pool.
#[injectable]
#[derive(Default)]
struct AdmitRequest;

impl Layer for AdmitRequest {}

#[async_trait]
impl Guard for AdmitRequest {
    async fn check_http(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        nest_rs_core::__private::set_actor_id("admitted");
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for AdmitRequest {}

#[mcp(path = "/mcp/declared")]
#[derive(Clone, Default)]
struct DeclaredTool;

#[tools]
impl DeclaredTool {
    /// Answer with a constant.
    #[tool]
    #[public]
    #[use_guards(DeclaredEverywhere)]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

#[module(providers = [
    DeclaredTool,
    DeclaredEverywhere,
    AdmitRequest,
    nest_rs_http::EndpointGuards<nest_rs_mcp::McpView, (AdmitRequest,)>,
])]
struct DeclaredModule;

#[tokio::test]
async fn a_guard_the_edge_did_not_run_still_runs_per_operation() {
    let app = TestApp::builder()
        .use_guards_global([nest_rs_guards::guard::<DeclaredEverywhere>()])
        .module::<DeclaredModule>()
        .build()
        .await
        .expect("endpoint guards plus a guard declared both globally and on the tool boot");

    let body = call_tool(app.http(), "/mcp/declared", "ping", None).await;
    assert!(body.contains("pong"), "the operation ran: {body}");
    assert_eq!(
        DECLARED_CALLS.load(Ordering::SeqCst),
        1,
        "the request's guards run `check_http` alone, so the guard declared \
         beside this tool must still execute its `check` — dropping it because \
         its `TypeId` also appears in the pool is a fail-open on a declaration \
         the developer wrote",
    );
}

/// Listed by a host's `#[use_guards]`, provided by no module.
#[injectable]
#[derive(Default)]
struct NeverProvided;

impl Layer for NeverProvided {}

#[async_trait]
impl Guard for NeverProvided {
    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_mcp::McpView> for NeverProvided {}

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

    use nest_rs_http::Endpoint as _;

    // Built by hand, so the access graph does not refuse first; the request
    // scope stands in for the edge that installs it.
    let container = nest_rs_core::Container::builder()
        .build()
        .expect("the container builds");
    let inner =
        nest_rs_mcp::__private::endpoint(nest_rs_mcp::__private::McpMount::public(), || OrphanTool);
    let scoped = nest_rs_http::endpoint_fn(move |req| {
        let inner = inner.clone();
        let scope = Arc::new(nest_rs_core::RequestScope::new(container.clone()));
        let correlation = nest_rs_core::Correlation::minted(None);
        async move { nest_rs_core::with_request_scope(Some(scope), correlation, inner.call(req)).await }
    });
    let client = nest_rs_testing::TestClient::new(scoped);
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

/// `static` counter: this test's alone, nextest running each in its own process.
static DROPPED: AtomicUsize = AtomicUsize::new(0);

#[injectable]
#[derive(Default)]
struct CountsItsDrop;

impl Drop for CountsItsDrop {
    fn drop(&mut self) {
        DROPPED.fetch_add(1, Ordering::SeqCst);
    }
}

impl Layer for CountsItsDrop {}

#[async_trait]
impl Guard for CountsItsDrop {
    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_mcp::McpView> for CountsItsDrop {}

#[mcp(path = "/mcp/counted")]
#[use_guards(CountsItsDrop)]
#[derive(Clone, Default)]
struct CountedTool;

#[tools]
impl CountedTool {
    /// Answer with a constant.
    #[tool]
    #[public]
    async fn count(&self) -> Result<String, McpError> {
        Ok("counted".to_owned())
    }
}

#[module(imports = [crate::open_mcp()], providers = [
    CountedTool,
    CountsItsDrop])]
struct CountedModule;

#[tokio::test]
async fn dropping_the_app_drops_the_guards_its_operations_composed() {
    let app = TestApp::for_module::<CountedModule>().await.expect("boots");
    let body = call_tool(app.http(), "/mcp/counted", "count", None).await;
    assert!(body.contains("counted"), "the guarded tool ran: {body}");
    assert_eq!(DROPPED.load(Ordering::SeqCst), 0, "the app is alive");

    drop(app);

    assert_eq!(
        DROPPED.load(Ordering::SeqCst),
        1,
        "no `static` keeps the composed chain, so the guard went with its app",
    );
}

// The operation's site: what `#[tools]` declares for each operation runs on
// core's pipeline — the guard stage and the class gate, the exception filters,
// the handler stage — inside the operation's unit.

/// What each interceptor ran around, as `<method> <name>`.
#[injectable]
#[derive(Default)]
struct AroundLog {
    seen: std::sync::Mutex<Vec<String>>,
}

impl AroundLog {
    fn record(&self, cx: &nest_rs_core::UnitContext<'_>) {
        let view = cx
            .switch_to::<nest_rs_mcp::McpView>()
            .expect("an MCP unit carries the MCP view");
        let seen = format!("{} {}", view.method(), view.name().unwrap_or("-"));
        self.seen.lock().expect("the log").push(seen);
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.seen.lock().expect("the log"))
    }
}

/// Writes down each unit it runs around, then runs it.
#[injectable]
struct Around {
    #[inject]
    log: std::sync::Arc<AroundLog>,
}

impl Layer for Around {}

#[async_trait]
impl nest_rs_core::Interceptor for Around {
    async fn intercept(
        &self,
        cx: &mut nest_rs_core::UnitContext<'_>,
        next: nest_rs_core::Next<'_>,
    ) -> nest_rs_core::Outcome {
        self.log.record(cx);
        next.run(cx).await
    }
}

const INTERCEPTED: &str = "/mcp/intercepted";

#[mcp(path = "/mcp/intercepted")]
#[use_interceptors(Around)]
#[derive(Clone, Default)]
struct InterceptedTools;

#[tools]
impl InterceptedTools {
    /// Answer with a constant.
    #[tool]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }

    /// A prompt is an operation of the host like a tool.
    #[prompt]
    #[public]
    async fn greet(&self) -> Result<nest_rs_mcp::model::GetPromptResult, McpError> {
        Ok(nest_rs_mcp::model::GetPromptResult::new(vec![
            nest_rs_mcp::model::PromptMessage::new_text(nest_rs_mcp::model::Role::User, "hi"),
        ]))
    }
}

#[module(imports = [crate::open_mcp()], providers = [InterceptedTools, Around, AroundLog])]
struct InterceptedModule;

#[tokio::test]
async fn a_hosts_interceptor_wraps_a_tool_call_once_and_no_listing() {
    let app = TestApp::for_module::<InterceptedModule>()
        .await
        .expect("a host binding an interceptor boots");
    let log = app.container().get::<AroundLog>().expect("provided");
    let session = open_session(app.http(), INTERCEPTED, None).await;
    log.take();

    let called = call_method(
        app.http(),
        INTERCEPTED,
        &session,
        None,
        "tools/call",
        json!({ "name": "ping", "arguments": {} }),
    )
    .await;
    assert!(called.contains("pong"), "the tool answered: {called}");
    assert_eq!(log.take(), ["tools/call ping"], "once around the call");

    let listed = call_method(
        app.http(),
        INTERCEPTED,
        &session,
        None,
        "tools/list",
        json!({}),
    )
    .await;
    assert!(listed.contains("ping"), "the listing answered: {listed}");
    assert!(
        log.take().is_empty(),
        "a host's interceptor runs around its operations, never a listing"
    );

    call_method(
        app.http(),
        INTERCEPTED,
        &session,
        None,
        "prompts/get",
        json!({ "name": "greet", "arguments": {} }),
    )
    .await;
    assert_eq!(log.take(), ["prompts/get greet"], "a prompt is wrapped too");
}

#[module(imports = [crate::open_mcp()], providers = [LayeredTool, Recorder, Closed, crate::StoreDown, Around, AroundLog])]
struct GloballyInterceptedModule;

/// The app's own interceptors reach each tool call and prompt fetch, at the
/// handler stage, after the operation's guards admitted it.
#[tokio::test]
async fn the_apps_interceptors_run_around_each_operation_its_guards_admit() {
    let app = TestApp::builder()
        .use_interceptors_global([nest_rs_core::interceptor::<Around>()])
        .module::<GloballyInterceptedModule>()
        .build()
        .await
        .expect("an app-wide interceptor beside an MCP host boots");
    let log = app.container().get::<AroundLog>().expect("provided");
    let session = open_session(app.http(), PATH, None).await;
    log.take();

    let opened = call_method(
        app.http(),
        PATH,
        &session,
        None,
        "tools/call",
        json!({ "name": "open", "arguments": {} }),
    )
    .await;
    assert!(opened.contains("open"), "{opened}");
    assert_eq!(log.take(), ["tools/call open"]);

    let refused = call_method(
        app.http(),
        PATH,
        &session,
        None,
        "tools/call",
        json!({ "name": "shut", "arguments": {} }),
    )
    .await;
    assert!(refused.contains("not for you"), "{refused}");
    assert!(
        log.take().is_empty(),
        "a refused caller meets no handler-stage interceptor"
    );

    call_method(
        app.http(),
        PATH,
        &session,
        None,
        "prompts/get",
        json!({ "name": "draft", "arguments": {} }),
    )
    .await;
    assert_eq!(log.take(), ["prompts/get draft"]);

    call_method(app.http(), PATH, &session, None, "tools/list", json!({})).await;
    assert!(log.take().is_empty(), "a listing runs no handler stage");
}

/// What a ledger answers when it refuses a charge: the domain's own error,
/// which no MCP type names.
#[derive(Debug, thiserror::Error)]
#[error("the ledger holds account 4111-1111 frozen")]
struct LedgerRefused;

/// Maps the ledger's refusal to a `409`.
#[injectable]
#[derive(Default)]
struct LedgerConflict;

impl Layer for LedgerConflict {}

#[async_trait]
impl nest_rs_core::ExceptionFilter for LedgerConflict {
    type Exception = LedgerRefused;

    async fn catch(
        &self,
        _: &LedgerRefused,
        _: &mut nest_rs_core::UnitContext<'_>,
    ) -> nest_rs_core::Caught {
        nest_rs_core::Caught::Problem(nest_rs_core::Problem::new(
            409,
            nest_rs_core::problem::code::CONFLICT,
        ))
    }
}

const LEDGER: &str = "/mcp/ledger";

#[mcp(path = "/mcp/ledger")]
#[derive(Clone, Default)]
struct LedgerTools;

#[tools]
impl LedgerTools {
    /// Charge the account — the ledger refuses it.
    #[tool]
    #[public]
    #[use_exception_filters(LedgerConflict)]
    async fn charge(&self) -> Result<String, LedgerRefused> {
        Err(LedgerRefused)
    }

    /// Refund the account — the ledger refuses it, and nothing maps the refusal.
    #[tool]
    #[public]
    async fn refund(&self) -> Result<String, LedgerRefused> {
        Err(LedgerRefused)
    }
}

#[module(imports = [crate::open_mcp()], providers = [LedgerTools, LedgerConflict])]
struct LedgerModule;

#[tokio::test]
async fn an_exception_filter_maps_a_tools_typed_error_to_a_jsonrpc_error_with_its_code() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<LedgerModule>()
        .await
        .expect("a tool binding an exception filter boots");

    let body = call_tool(app.http(), LEDGER, "charge", None).await;
    let answer = nest_rs_testing::mcp::result(&body);
    assert_eq!(
        answer["error"]["code"],
        json!(-32600),
        "a 4xx is the caller's: {answer}"
    );
    assert_eq!(answer["error"]["data"]["code"], "CONFLICT", "{answer}");
    assert!(
        !body.contains("4111"),
        "the domain error's words stay home: {body}"
    );
    logs.expect_none(nest_rs_mcp::TARGET, "mcp operation failed");

    let unmapped = call_tool(app.http(), LEDGER, "refund", None).await;
    let answer = nest_rs_testing::mcp::result(&unmapped);
    assert_eq!(
        answer["error"]["message"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "an error no filter answers and no type says is opaque: {answer}",
    );
    assert!(!unmapped.contains("4111"), "{unmapped}");
    let failed = logs.expect_one(nest_rs_mcp::TARGET, "mcp operation failed");
    assert_eq!(failed.level, "error");
    assert!(
        failed
            .field("error")
            .is_some_and(|chain| chain.contains("frozen")),
        "the operator reads the chain: {:?}",
        failed.fields,
    );
}

/// The ledger's `Result` behind an alias: the expansion cannot spell its value.
type Ledgered<T> = Result<T, LedgerRefused>;

const ALIASED_LEDGER: &str = "/mcp/aliased-ledger";

#[mcp(path = "/mcp/aliased-ledger")]
#[derive(Clone, Default)]
struct AliasedLedgerTools;

#[tools]
impl AliasedLedgerTools {
    /// Report the balance through the alias.
    #[tool]
    #[public]
    async fn balance(&self) -> Ledgered<String> {
        Ok("0.00".to_owned())
    }

    /// Charge the account through the alias — the ledger refuses it.
    #[tool]
    #[public]
    #[use_exception_filters(LedgerConflict)]
    async fn charge(&self) -> Ledgered<String> {
        Err(LedgerRefused)
    }

    /// Refund the account through the alias — nothing maps the refusal.
    #[tool]
    #[public]
    async fn refund(&self) -> Ledgered<String> {
        Err(LedgerRefused)
    }

    /// Draft the account's statement through the alias — the ledger refuses it.
    #[prompt]
    #[public]
    #[use_exception_filters(LedgerConflict)]
    async fn statement(&self) -> Ledgered<nest_rs_mcp::model::GetPromptResult> {
        Err(LedgerRefused)
    }
}

#[module(imports = [crate::open_mcp()], providers = [AliasedLedgerTools, LedgerConflict])]
struct AliasedLedgerModule;

/// A `Result` behind an alias answers as the one spelled out, whatever error
/// it fails with: read by its type, through a tool and a prompt alike.
#[tokio::test]
async fn an_operation_answering_through_an_alias_answers_as_the_spelled_result() {
    let app = TestApp::for_module::<AliasedLedgerModule>()
        .await
        .expect("operations answering an aliased `Result` boot");

    let balance = call_tool(app.http(), ALIASED_LEDGER, "balance", None).await;
    assert!(balance.contains("0.00"), "{balance}");

    let charged = call_tool(app.http(), ALIASED_LEDGER, "charge", None).await;
    let answer = nest_rs_testing::mcp::result(&charged);
    assert_eq!(answer["error"]["data"]["code"], "CONFLICT", "{answer}");

    let refunded = call_tool(app.http(), ALIASED_LEDGER, "refund", None).await;
    let answer = nest_rs_testing::mcp::result(&refunded);
    assert_eq!(
        answer["error"]["message"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "{answer}",
    );
    assert!(!refunded.contains("4111"), "{refunded}");

    let session = open_session(app.http(), ALIASED_LEDGER, None).await;
    let drafted = call_method(
        app.http(),
        ALIASED_LEDGER,
        &session,
        None,
        "prompts/get",
        json!({ "name": "statement", "arguments": {} }),
    )
    .await;
    let answer = nest_rs_testing::mcp::result(&drafted);
    assert_eq!(answer["error"]["data"]["code"], "CONFLICT", "{answer}");
}

// Each scope's exception filter and interceptor on a tool: the method's filter
// answers first, then the host's, then the app's; the app's interceptor runs
// outermost, then the host's, then the method's.

/// What a vault answers when it stays shut: the domain's own error.
#[derive(Debug, thiserror::Error)]
#[error("the vault at 10.0.0.9 is sealed")]
struct Sealed;

/// Answers [`Sealed`] with `code`.
fn sealed_as(status: u16, code: nest_rs_core::problem::Code) -> nest_rs_core::Caught {
    nest_rs_core::Caught::Problem(nest_rs_core::Problem::new(status, code))
}

/// The method's filter: a `409`.
#[injectable]
#[derive(Default)]
struct SealedAtMethod;

impl Layer for SealedAtMethod {}

#[async_trait]
impl nest_rs_core::ExceptionFilter for SealedAtMethod {
    type Exception = Sealed;

    async fn catch(
        &self,
        _: &Sealed,
        _: &mut nest_rs_core::UnitContext<'_>,
    ) -> nest_rs_core::Caught {
        sealed_as(409, nest_rs_core::problem::code::CONFLICT)
    }
}

/// The host's filter: a `404`.
#[injectable]
#[derive(Default)]
struct SealedAtHost;

impl Layer for SealedAtHost {}

#[async_trait]
impl nest_rs_core::ExceptionFilter for SealedAtHost {
    type Exception = Sealed;

    async fn catch(
        &self,
        _: &Sealed,
        _: &mut nest_rs_core::UnitContext<'_>,
    ) -> nest_rs_core::Caught {
        sealed_as(404, nest_rs_core::problem::code::NOT_FOUND)
    }
}

/// The app's filter: a `400`.
#[injectable]
#[derive(Default)]
struct SealedAtApp;

impl Layer for SealedAtApp {}

#[async_trait]
impl nest_rs_core::ExceptionFilter for SealedAtApp {
    type Exception = Sealed;

    async fn catch(
        &self,
        _: &Sealed,
        _: &mut nest_rs_core::UnitContext<'_>,
    ) -> nest_rs_core::Caught {
        sealed_as(400, nest_rs_core::problem::code::INVALID_ARGUMENT)
    }
}

/// Which scope's interceptor ran, in the order they ran.
#[injectable]
#[derive(Default)]
struct ScopeLog {
    seen: std::sync::Mutex<Vec<&'static str>>,
}

impl ScopeLog {
    fn take(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.seen.lock().expect("the log"))
    }
}

/// An interceptor writing down `scope` before it runs the rest.
macro_rules! scope_interceptor {
    ($name:ident, $scope:literal) => {
        #[injectable]
        struct $name {
            #[inject]
            log: std::sync::Arc<ScopeLog>,
        }

        impl Layer for $name {}

        #[async_trait]
        impl nest_rs_core::Interceptor for $name {
            async fn intercept(
                &self,
                cx: &mut nest_rs_core::UnitContext<'_>,
                next: nest_rs_core::Next<'_>,
            ) -> nest_rs_core::Outcome {
                self.log.seen.lock().expect("the log").push($scope);
                next.run(cx).await
            }
        }
    };
}

scope_interceptor!(AroundApp, "app");
scope_interceptor!(AroundHost, "host");
scope_interceptor!(AroundMethod, "method");

const VAULT: &str = "/mcp/vault";

#[mcp(path = "/mcp/vault")]
#[use_exception_filters(SealedAtHost)]
#[use_interceptors(AroundHost)]
#[derive(Clone, Default)]
struct VaultTools;

#[tools]
impl VaultTools {
    /// Open the vault, under a filter and an interceptor of its own.
    #[tool]
    #[public]
    #[use_exception_filters(SealedAtMethod)]
    #[use_interceptors(AroundMethod)]
    async fn open_here(&self) -> Result<String, Sealed> {
        Err(Sealed)
    }

    /// Open the vault under the host's layers.
    #[tool]
    #[public]
    async fn open_hosted(&self) -> Result<String, Sealed> {
        Err(Sealed)
    }
}

const BARE_VAULT: &str = "/mcp/bare-vault";

#[mcp(path = "/mcp/bare-vault")]
#[derive(Clone, Default)]
struct BareVaultTools;

#[tools]
impl BareVaultTools {
    /// Open the vault under the app's layers alone.
    #[tool]
    #[public]
    async fn open_bare(&self) -> Result<String, Sealed> {
        Err(Sealed)
    }
}

#[module(imports = [crate::open_mcp()], providers = [
    VaultTools, BareVaultTools, SealedAtMethod, SealedAtHost, SealedAtApp,
    ScopeLog, AroundApp, AroundHost, AroundMethod,
])]
struct VaultModule;

/// The `data.code` a call of `tool` on `path` is refused with.
async fn sealed_code(app: &TestApp, path: &str, tool: &str) -> serde_json::Value {
    let body = call_tool(app.http(), path, tool, None).await;
    assert!(
        !body.contains("10.0.0.9"),
        "the domain error stays home: {body}"
    );
    nest_rs_testing::mcp::result(&body)["error"]["data"]["code"].clone()
}

#[tokio::test]
async fn each_scopes_filter_and_interceptor_reach_a_tool_in_scope_order() {
    let app = TestApp::builder()
        .use_exception_filters_global([nest_rs_core::exception_filter::<SealedAtApp>()])
        .use_interceptors_global([nest_rs_core::interceptor::<AroundApp>()])
        .module::<VaultModule>()
        .build()
        .await
        .expect("a filter and an interceptor at every scope boot");
    let log = app.container().get::<ScopeLog>().expect("provided");

    assert_eq!(
        sealed_code(&app, VAULT, "open_here").await,
        "CONFLICT",
        "the method's filter first",
    );
    assert_eq!(log.take(), ["app", "host", "method"]);

    assert_eq!(
        sealed_code(&app, VAULT, "open_hosted").await,
        "NOT_FOUND",
        "then the host's",
    );
    assert_eq!(log.take(), ["app", "host"]);

    assert_eq!(
        sealed_code(&app, BARE_VAULT, "open_bare").await,
        "INVALID_ARGUMENT",
        "then the app's",
    );
    assert_eq!(log.take(), ["app"]);
}
