//! `src/propagate.rs`: every `ServerHandler` method reaches the wrapped host —
//! a dropped delegation is answered by rmcp's default, silently — and an
//! operation ends when it is stopped as well as when it settles.
//!
//! This proves delegation, not exhaustiveness: `#[deny(clippy::missing_trait_methods)]`
//! on the wrapper does that. `negotiate_initialize` is absent from [`EXPECTED`]:
//! the wire cannot tell it delegated, so `src/propagate.rs` proves it.

#![expect(
    deprecated,
    reason = "rmcp still routes the deprecated methods for legacy protocol versions"
)]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nest_rs_core::module;

use nest_rs_mcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams,
    CancelledNotificationParam, CompleteRequestParams, CompleteResult, ContentBlock,
    CustomNotification, CustomRequest, CustomResult, DiscoverResult, GetPromptRequestParams,
    GetPromptResponse, GetPromptResult, GetTaskParams, GetTaskResult, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProgressNotificationParam, ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, ResourceContents, ServerCapabilities, ServerConfig, SetLevelRequestParams,
    SubscribeRequestParams, SubscriptionFilter, Tool, UnsubscribeRequestParams, UpdateTaskParams,
};
use nest_rs_mcp::rmcp::serde_json::{self, json};
use nest_rs_mcp::service::{NotificationContext, RequestContext, RoleServer};
use nest_rs_mcp::{
    AllowAllMcpGuard, McpError, McpMount, McpOperationGuard, PropagatingHandler, ServerHandler,
    endpoint, mcp, tools,
};
use nest_rs_testing::mcp::{call_method, notify, open_session, open_session_with};
use nest_rs_testing::{LogCapture, TestApp};
use poem::test::TestClient;
use tokio::sync::Notify;

type Seen = Arc<Mutex<BTreeSet<&'static str>>>;

/// Client capabilities declaring the SEP-2663 tasks extension, without which
/// `tasks/*` is refused before it ever reaches a handler.
fn tasks_client_capabilities() -> serde_json::Value {
    json!({ "extensions": { "io.modelcontextprotocol/tasks": {} } })
}

/// Records which `ServerHandler` method ran. `initialize` is not overridden:
/// rmcp's default negotiates through a `pub(crate)` helper a host cannot call.
#[derive(Clone)]
struct ProbeHandler {
    seen: Seen,
}

impl ProbeHandler {
    fn mark(&self, name: &'static str) {
        self.seen.lock().expect("probe lock").insert(name);
    }
}

impl ServerHandler for ProbeHandler {
    fn get_info(&self) -> ServerConfig {
        self.mark("get_info");
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .enable_resources_subscribe()
                .enable_logging()
                .enable_completions()
                .enable_tasks()
                .build(),
        )
    }

    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        self.mark("supported_protocol_versions");
        std::borrow::Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS)
    }

    fn get_tool(&self, _name: &str) -> Option<Tool> {
        self.mark("get_tool");
        None
    }

    fn accepted_subscription_filter(
        &self,
        _requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        self.mark("accepted_subscription_filter");
        // Accepting routes the request on to `listen`.
        Some(_requested.clone())
    }

    /// Returns at once, so the request is not held open.
    async fn listen(
        &self,
        _context: nest_rs_mcp::service::SubscriptionContext,
    ) -> Result<(), McpError> {
        self.mark("listen");
        Ok(())
    }

    async fn ping(&self, _context: RequestContext<RoleServer>) -> Result<(), McpError> {
        self.mark("ping");
        Ok(())
    }

    async fn discover(
        &self,
        _context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, McpError> {
        self.mark("discover");
        Ok(DiscoverResult::from_server_info(
            self.supported_protocol_versions().into_owned(),
            self.get_info(),
        ))
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        self.mark("call_tool");
        Ok(CallToolResult::success(vec![ContentBlock::text("probe")]).into())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        self.mark("list_tools");
        Ok(ListToolsResult::default())
    }

    async fn get_prompt(
        &self,
        _request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        self.mark("get_prompt");
        Ok(GetPromptResult::new(Vec::new())
            .with_description("probe")
            .into())
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        self.mark("list_prompts");
        Ok(ListPromptsResult::default())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        self.mark("read_resource");
        Ok(ReadResourceResult::new(vec![ResourceContents::text("probe", request.uri)]).into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        self.mark("list_resources");
        Ok(ListResourcesResult::default())
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        self.mark("list_resource_templates");
        Ok(ListResourceTemplatesResult::default())
    }

    async fn subscribe(
        &self,
        _request: SubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.mark("subscribe");
        Ok(())
    }

    async fn unsubscribe(
        &self,
        _request: UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.mark("unsubscribe");
        Ok(())
    }

    async fn complete(
        &self,
        _request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, McpError> {
        self.mark("complete");
        Ok(CompleteResult::default())
    }

    async fn set_level(
        &self,
        _request: SetLevelRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.mark("set_level");
        Ok(())
    }

    async fn get_task(
        &self,
        _request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, McpError> {
        self.mark("get_task");
        Err(McpError::internal_error("probe".to_string(), None))
    }

    async fn update_task(
        &self,
        _request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.mark("update_task");
        Ok(())
    }

    async fn cancel_task(
        &self,
        _request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.mark("cancel_task");
        Ok(())
    }

    async fn on_custom_request(
        &self,
        _request: CustomRequest,
        _context: RequestContext<RoleServer>,
    ) -> Result<CustomResult, McpError> {
        self.mark("on_custom_request");
        Ok(CustomResult(json!({ "probe": true })))
    }

    async fn on_cancelled(
        &self,
        _notification: CancelledNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) {
        self.mark("on_cancelled");
    }

    async fn on_progress(
        &self,
        _notification: ProgressNotificationParam,
        _context: NotificationContext<RoleServer>,
    ) {
        self.mark("on_progress");
    }

    async fn on_initialized(&self, _context: NotificationContext<RoleServer>) {
        self.mark("on_initialized");
    }

    async fn on_roots_list_changed(&self, _context: NotificationContext<RoleServer>) {
        self.mark("on_roots_list_changed");
    }

    async fn on_custom_notification(
        &self,
        _notification: CustomNotification,
        _context: NotificationContext<RoleServer>,
    ) {
        self.mark("on_custom_notification");
    }
}

const EXPECTED: &[&str] = &[
    "accepted_subscription_filter",
    "call_tool",
    "cancel_task",
    "complete",
    "discover",
    "get_info",
    "get_prompt",
    "get_task",
    "get_tool",
    "list_prompts",
    "list_resource_templates",
    "list_resources",
    "list_tools",
    "listen",
    "on_cancelled",
    "on_custom_notification",
    "on_custom_request",
    "on_initialized",
    "on_progress",
    "on_roots_list_changed",
    "ping",
    "read_resource",
    "set_level",
    "subscribe",
    "supported_protocol_versions",
    "unsubscribe",
    "update_task",
];

/// The revision carrying SEP-2575 discovery/subscriptions and SEP-2243 headers.
const MODERN_VERSION: &str = "2026-07-28";

/// rmcp accepts every older revision forever, so nothing else fails when the
/// stateless suite falls behind.
#[test]
fn the_stateless_suite_drives_the_sdk_latest() {
    assert_eq!(
        MODERN_VERSION,
        ProtocolVersion::LATEST.as_str(),
        "rmcp moved its LATEST: bump `MODERN_VERSION` to match",
    );
}

/// Per-request `_meta` a modern (stateless, inline-lifecycle) request must
/// carry, per SEP-2575.
fn modern_meta() -> serde_json::Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": MODERN_VERSION,
        "io.modelcontextprotocol/clientInfo": { "name": "nest-rs-mcp", "version": "0" },
        "io.modelcontextprotocol/clientCapabilities": {
            "extensions": { "io.modelcontextprotocol/tasks": {} }
        },
    })
}

/// Notifications are answered `202` and processed on the session worker, so the
/// recorder lags the POST.
async fn await_seen(seen: &Seen, expected: &[&str]) {
    for _ in 0..200 {
        let current = seen.lock().expect("probe lock").clone();
        if expected.iter().all(|name| current.contains(name)) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn every_server_handler_method_reaches_the_wrapped_host() {
    let seen: Seen = Arc::default();
    let host = ProbeHandler { seen: seen.clone() };

    let guard = Arc::new(AllowAllMcpGuard) as Arc<dyn McpOperationGuard>;
    let client = TestClient::new(endpoint(
        McpMount::deny_all().with_guard(guard),
        move || host.clone(),
    ));

    // A legacy session: the capabilities reachable through `initialize`.
    let session = open_session_with(&client, "/", None, &[], tasks_client_capabilities()).await;

    let requests: &[(&str, serde_json::Value)] = &[
        ("ping", json!({})),
        ("tools/list", json!({})),
        ("tools/call", json!({ "name": "probe", "arguments": {} })),
        ("prompts/list", json!({})),
        ("prompts/get", json!({ "name": "probe" })),
        ("resources/list", json!({})),
        ("resources/templates/list", json!({})),
        ("resources/read", json!({ "uri": "probe://one" })),
        ("resources/subscribe", json!({ "uri": "probe://one" })),
        ("resources/unsubscribe", json!({ "uri": "probe://one" })),
        (
            "completion/complete",
            json!({
                "ref": { "type": "ref/prompt", "name": "probe" },
                "argument": { "name": "arg", "value": "" }
            }),
        ),
        ("logging/setLevel", json!({ "level": "info" })),
        ("tasks/get", json!({ "taskId": "probe" })),
        ("tasks/cancel", json!({ "taskId": "probe" })),
        (
            "tasks/update",
            json!({ "taskId": "probe", "inputResponses": {} }),
        ),
        ("probe/custom", json!({})),
    ];
    for (method, params) in requests {
        call_method(&client, "/", &session, None, method, params.clone()).await;
    }

    // `notifications/initialized` already ran inside the handshake.
    let notifications: &[(&str, serde_json::Value)] = &[
        (
            "notifications/cancelled",
            json!({ "requestId": 99, "reason": "probe" }),
        ),
        (
            "notifications/progress",
            json!({ "progressToken": "probe", "progress": 1 }),
        ),
        ("notifications/roots/list_changed", json!({})),
        ("notifications/probe", json!({})),
    ];
    for (method, params) in notifications {
        notify(&client, "/", &session, None, method, params.clone()).await;
    }

    // `server/discover`, `subscriptions/listen` and `get_tool` (consulted only to
    // validate SEP-2243 `Mcp-Param-*` headers) need stateless 2026-07-28 requests.
    let modern: &[(&str, serde_json::Value)] = &[
        ("server/discover", json!({ "_meta": modern_meta() })),
        (
            "subscriptions/listen",
            json!({ "notifications": {}, "_meta": modern_meta() }),
        ),
    ];
    for (method, params) in modern {
        post_modern(&client, method, params.clone(), &[]).await;
    }
    post_modern(
        &client,
        "tools/call",
        json!({ "name": "probe", "arguments": {}, "_meta": modern_meta() }),
        &[("mcp-name", "probe")],
    )
    .await;

    await_seen(&seen, EXPECTED).await;

    let seen = seen.lock().expect("probe lock").clone();
    let missing: Vec<&str> = EXPECTED
        .iter()
        .copied()
        .filter(|name| !seen.contains(name))
        .collect();

    assert!(
        missing.is_empty(),
        "PropagatingHandler did not delegate {missing:?} — rmcp's default answered for the host \
         instead. Add the method to the delegation in `src/propagate.rs`. Reached: {seen:?}",
    );
}

/// POST one stateless `2026-07-28` request: no session id, the negotiated
/// version in the header, and whatever SEP-2243 standard headers the case needs.
async fn post_modern<E: poem::Endpoint>(
    client: &TestClient<E>,
    method: &str,
    params: serde_json::Value,
    headers: &[(&str, &str)],
) {
    let mut request = client
        .post("/")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", MODERN_VERSION)
        // SEP-2243 makes `Mcp-Method` mandatory once the negotiated version
        // carries standard headers.
        .header("mcp-method", method)
        .body_json(&json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": method,
            "params": params,
        }));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.send().await;
}

/// `StreamableHttpService` bounds on `ServerHandler`.
#[test]
fn the_wrapper_is_itself_a_server_handler() {
    fn assert_server_handler<T: ServerHandler>() {}
    assert_server_handler::<PropagatingHandler<ProbeHandler>>();
}

/// Set when `slow`'s future is dropped, finished, and started, in turn.
static SLOW_DROPPED: AtomicBool = AtomicBool::new(false);
static SLOW_FINISHED: AtomicBool = AtomicBool::new(false);
static SLOW_STARTED: Notify = Notify::const_new();
/// The same three for `held`.
static HELD_DROPPED: AtomicBool = AtomicBool::new(false);
static HELD_FINISHED: AtomicBool = AtomicBool::new(false);
static HELD_STARTED: Notify = Notify::const_new();

/// Sets its flag when dropped — which is what stopping an operation is.
struct SetOnDrop(&'static AtomicBool);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

const STOPPING: &str = "/mcp/stopping";

#[mcp(path = "/mcp/stopping")]
#[derive(Clone, Default)]
struct StoppingTools;

#[tools]
impl StoppingTools {
    #[tool(description = "Outlasts the shutdown window it is served under.")]
    #[public]
    async fn slow(&self) -> Result<String, McpError> {
        let _dropped = SetOnDrop(&SLOW_DROPPED);
        SLOW_STARTED.notify_one();
        tokio::time::sleep(Duration::from_secs(30)).await;
        SLOW_FINISHED.store(true, Ordering::SeqCst);
        Ok("finished".to_owned())
    }

    #[tool(description = "Waits until its client gives up on it.")]
    #[public]
    async fn held(&self) -> Result<String, McpError> {
        let _dropped = SetOnDrop(&HELD_DROPPED);
        HELD_STARTED.notify_one();
        tokio::time::sleep(Duration::from_secs(30)).await;
        HELD_FINISHED.store(true, Ordering::SeqCst);
        Ok("finished".to_owned())
    }
}

#[module(providers = [StoppingTools, AllowAllMcpGuard as dyn McpOperationGuard])]
struct StoppingModule;

/// The one `mcp.operation` line filed for `operation`, once it is filed.
async fn operation_line(logs: &LogCapture, operation: &str) -> nest_rs_testing::CapturedEvent {
    for _ in 0..200 {
        let filed: Vec<_> = logs
            .find(
                nest_rs_core::operation_log::TARGET,
                nest_rs_mcp::unit::OPERATION.name(),
            )
            .into_iter()
            .filter(|line| line.field("operation").as_deref() == Some(operation))
            .collect();
        match filed.as_slice() {
            [] => tokio::time::sleep(Duration::from_millis(10)).await,
            [line] => return line.clone(),
            _ => panic!("one line per operation, got {filed:?}"),
        }
    }
    panic!("no mcp.operation line was filed for {operation}");
}

/// rmcp runs an operation on a task of its own, which outlives the connection
/// the shutdown window cuts unless the transport stops it.
#[tokio::test]
async fn an_operation_running_when_the_transport_stops_is_dropped_and_files_cancelled() {
    let logs = LogCapture::install();
    let window = Duration::from_secs(1);
    let serving = crate::serve_on_loopback::<StoppingModule>(window).await;
    let port = serving.port;
    let session = crate::open_raw_session(port, STOPPING).await;
    let (_open, head) = crate::post(
        port,
        STOPPING,
        Some(&session),
        &json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "slow", "arguments": {} }
        }),
    )
    .await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "the call is accepted: {head}"
    );
    tokio::time::timeout(Duration::from_secs(5), SLOW_STARTED.notified())
        .await
        .expect("the tool starts");

    let took = serving.stop().await;

    assert!(
        SLOW_DROPPED.load(Ordering::SeqCst) && !SLOW_FINISHED.load(Ordering::SeqCst),
        "the operation was dropped where it waited before the transport returned",
    );
    assert!(
        took >= window
            && took < window + nest_rs_core::SHUTDOWN_SETTLE_TIMEOUT + Duration::from_secs(1),
        "the transport stopped at its window, took {took:?}",
    );
    let line = operation_line(&logs, "slow").await;
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
    );
    let stopped = logs.expect_one(
        nest_rs_http::target::HTTP,
        "work a self-mount ran off its connections is stopped with the transport; a unit still \
         running is dropped unanswered",
    );
    assert_eq!(stopped.level, "warn");
    assert_eq!(stopped.field("path").as_deref(), Some(STOPPING));
    assert_eq!(stopped.field("stopped").as_deref(), Some("1"));
}

/// `notifications/cancelled` reaches rmcp as a token no handler is obliged to watch.
#[tokio::test]
async fn an_operation_its_client_cancels_is_dropped_and_files_cancelled() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<StoppingModule>()
        .await
        .expect("the module boots");
    let session = open_session(app.http(), STOPPING, None).await;

    let call = call_method(
        app.http(),
        STOPPING,
        &session,
        None,
        "tools/call",
        json!({ "name": "held", "arguments": {} }),
    );
    tokio::pin!(call);
    tokio::select! {
        body = &mut call => panic!("the call answered before it was cancelled: {body}"),
        () = HELD_STARTED.notified() => {}
    }
    // `call_method` sends every request as id 99.
    notify(
        app.http(),
        STOPPING,
        &session,
        None,
        "notifications/cancelled",
        json!({ "requestId": 99, "reason": "the user gave up" }),
    )
    .await;

    let line = operation_line(&logs, "held").await;
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
    );
    assert!(
        HELD_DROPPED.load(Ordering::SeqCst) && !HELD_FINISHED.load(Ordering::SeqCst),
        "the operation was dropped where it waited",
    );
}

const EXPLODING: &str = "/mcp/exploding";

#[mcp(path = "/mcp/exploding")]
#[derive(Clone, Default)]
struct ExplodingTools;

#[tools]
impl ExplodingTools {
    #[tool(description = "Unwinds instead of answering.")]
    #[public]
    async fn boom(&self) -> Result<String, McpError> {
        tokio::task::yield_now().await;
        panic!("the tool exploded with sk_live_secret in hand")
    }
}

#[module(providers = [ExplodingTools, AllowAllMcpGuard as dyn McpOperationGuard])]
struct ExplodingModule;

/// rmcp runs a tool on a task of its own: a panic there answers nothing unless
/// the dispatch contains it.
#[tokio::test]
async fn a_tool_that_panics_files_its_line_panic_and_its_client_is_answered() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<ExplodingModule>()
        .await
        .expect("the module boots");
    let session = open_session(app.http(), EXPLODING, None).await;

    let body = tokio::time::timeout(
        Duration::from_secs(5),
        call_method(
            app.http(),
            EXPLODING,
            &session,
            None,
            "tools/call",
            json!({ "name": "boom", "arguments": {} }),
        ),
    )
    .await
    .expect("the client is answered rather than left to its own timeout");

    let answer = nest_rs_testing::mcp::result(&body);
    assert_eq!(
        answer["error"]["message"],
        nest_rs_core::OPAQUE_CLIENT_MESSAGE,
        "an internal error, worded for nobody in particular: {answer}",
    );
    assert!(
        !body.contains("sk_live"),
        "nothing of the panic reaches the client: {body}"
    );
    let line = operation_line(&logs, "boom").await;
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
    );
    let span = logs
        .spans()
        .into_iter()
        .find(|span| {
            span.name == nest_rs_mcp::unit::OPERATION.name()
                && span.field("mcp.operation.name").as_deref() == Some("boom")
        })
        .expect("the operation's span");
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
        "{:?}",
        span.fields,
    );
    assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
    let contained = logs.expect_one(
        nest_rs_mcp::TARGET,
        "mcp operation panicked; its client is answered with an internal error",
    );
    assert_eq!(contained.level, "error");
    assert_eq!(contained.field("operation").as_deref(), Some("boom"));
    assert!(
        contained
            .field(nest_rs_core::panic::FIELD)
            .is_some_and(|panic| panic.contains("the tool exploded")),
        "the operator reads what unwound: {contained:#?}",
    );
}

const SUBSCRIBED: &str = "/mcp/subscribed";

/// A host whose subscription only its client ends: `listen` waits for the
/// cancellation and nothing else, as rmcp's own default does.
#[mcp(path = "/mcp/subscribed")]
#[derive(Clone)]
struct SubscribedHost;

#[nest_rs_mcp::tool_router(allow_empty)]
impl SubscribedHost {}

#[nest_rs_mcp::tool_handler]
impl ServerHandler for SubscribedHost {
    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        Some(requested.clone())
    }

    async fn listen(
        &self,
        context: nest_rs_mcp::service::SubscriptionContext,
    ) -> Result<(), McpError> {
        context.cancelled().await;
        Ok(())
    }
}

#[module(providers = [SubscribedHost, AllowAllMcpGuard as dyn McpOperationGuard])]
struct SubscribedModule;

/// The 2026-07-28 schema's graceful teardown is the final
/// `SubscriptionsListenResult`; the subscriber did not end it, so it files `cancelled`.
#[tokio::test]
async fn a_subscription_is_answered_its_final_result_at_the_shutdown_signal() {
    let logs = LogCapture::install();
    let window = Duration::from_secs(5);
    let serving = crate::serve_on_loopback::<SubscribedModule>(window).await;
    let body = json!({
        "jsonrpc": "2.0", "id": 42, "method": "subscriptions/listen",
        "params": { "notifications": {}, "_meta": modern_meta() },
    })
    .to_string();
    let (mut stream, head) = crate::exchange(
        serving.port,
        &format!(
            "POST {SUBSCRIBED} HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n\
             accept: application/json, text/event-stream\r\nmcp-protocol-version: {MODERN_VERSION}\r\n\
             mcp-method: subscriptions/listen\r\ncontent-length: {}\r\n\r\n{body}",
            body.len(),
        ),
    )
    .await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "the subscription opened: {head}"
    );
    let line = |logs: &LogCapture| {
        logs.find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_mcp::unit::OPERATION.name(),
        )
        .into_iter()
        .filter(|line| line.field("method").as_deref() == Some("subscriptions/listen"))
        .collect::<Vec<_>>()
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(line(&logs).is_empty(), "the subscription is running");

    let took = serving.stop().await;

    assert!(
        took < Duration::from_secs(1),
        "the subscription did not hold the {window:?} window, took {took:?}",
    );
    let rest = crate::read_to_end(&mut stream).await;
    let last = rest
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<serde_json::Value>(data).ok())
        .find(|message| message["id"] == 42)
        .unwrap_or_else(|| panic!("the final result reached the subscriber: {rest:?}"));
    assert!(
        last.get("result").is_some(),
        "a result, not an error: {last}"
    );
    let filed = line(&logs);
    assert_eq!(filed.len(), 1, "{filed:#?}");
    assert_eq!(
        filed[0].field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
    );
}
