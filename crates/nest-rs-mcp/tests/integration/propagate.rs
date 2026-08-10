//! `src/propagate.rs`: every `ServerHandler` method reaches the wrapped host.
//!
//! [`PropagatingHandler`](nest_rs_mcp::PropagatingHandler) has to *be* a
//! `ServerHandler` since rmcp 3.x — the single `Service::handle_request` seam it
//! used to wrap is gone. A method it forgets to delegate does not merely lose
//! the ambient request scope: rmcp's **default** answers instead, so the host's
//! own `prompts/get` becomes `-32601 Method not found` and its `tools/list`
//! becomes an empty list. Silently, and only on the wire.
//!
//! So this suite drives a probe host that records the name of every method it
//! is asked for, through the real streamable-HTTP endpoint, and asserts the
//! recorded set. Adding a capability to the delegation means adding it here.

// The probe implements the *whole* trait, deprecated members included: rmcp
// still routes legacy protocol versions to `subscribe`/`unsubscribe` and
// `logging/setLevel`, so a wrapper that drops them drops real traffic.
#![expect(deprecated)]

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use nest_rs_mcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams,
    CancelledNotificationParam, CompleteRequestParams, CompleteResult, ContentBlock,
    CustomNotification, CustomRequest, CustomResult, DiscoverResult, GetPromptRequestParams,
    GetPromptResponse, GetPromptResult, GetTaskParams, GetTaskResult, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProgressNotificationParam, ProtocolVersion, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, ResourceContents, ServerCapabilities, ServerInfo, SetLevelRequestParams,
    SubscribeRequestParams, SubscriptionFilter, Tool, UnsubscribeRequestParams, UpdateTaskParams,
};
use nest_rs_mcp::rmcp::serde_json::{self, json};
use nest_rs_mcp::service::{NotificationContext, RequestContext, RoleServer};
use nest_rs_mcp::{
    AllowAllMcpGuard, McpError, McpMount, McpOperationGuard, PropagatingHandler, ServerHandler,
    endpoint,
};
use nest_rs_testing::mcp::{call_method, notify, open_session_with};
use poem::test::TestClient;

/// The set of method names the probe host was asked for.
type Seen = Arc<Mutex<BTreeSet<&'static str>>>;

/// Client capabilities declaring the SEP-2663 tasks extension, without which
/// `tasks/*` is refused before it ever reaches a handler.
fn tasks_client_capabilities() -> serde_json::Value {
    json!({ "extensions": { "io.modelcontextprotocol/tasks": {} } })
}

/// A host that implements the **whole** `ServerHandler` surface and records
/// which method ran. `initialize` is deliberately not overridden: rmcp's
/// default performs protocol negotiation through a `pub(crate)` helper a host
/// cannot call, and a wrapper that dropped it would break the handshake this
/// whole suite depends on — the loudest possible failure, so it needs no probe.
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
    fn get_info(&self) -> ServerInfo {
        self.mark("get_info");
        ServerInfo::new(
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
        // Accepting is what routes the request on to `listen`; declining would
        // leave the wrapper's longest-lived delegation unexercised.
        Some(_requested.clone())
    }

    /// Returns at once rather than awaiting cancellation, so the suite proves
    /// the delegation without holding the request open.
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

/// Every method the wire exercises below are expected to reach on the host. A
/// delegation this wrapper drops makes the corresponding name go missing.
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

/// The revision that carries SEP-2575 discovery/subscriptions and SEP-2243
/// standard headers — the surface the legacy session below cannot reach.
const MODERN_VERSION: &str = "2026-07-28";

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
/// recorder lags the POST. Poll until the expected names land rather than
/// sleeping a guessed interval.
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

    // --- a legacy session: the capabilities reachable through `initialize` ---
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

    // Notifications. `notifications/initialized` already ran inside the
    // handshake; the rest are sent here.
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

    // --- the 2026-07-28 surface: stateless, inline-lifecycle requests -------
    // `server/discover` and `subscriptions/listen` do not exist for a legacy
    // session, and `get_tool` is only consulted to validate SEP-2243
    // `Mcp-Param-*` headers — all three are new in the revision this upgrade is
    // about, so leaving them unexercised would leave the new surface unproven.
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

/// The wrapper is a `ServerHandler`, which is what `StreamableHttpService`
/// bounds on since rmcp 3.x. Asserting it in a test keeps the bound from being
/// re-satisfied by accident (e.g. by an inherent method of the same name).
#[test]
fn the_wrapper_is_itself_a_server_handler() {
    fn assert_server_handler<T: ServerHandler>() {}
    assert_server_handler::<PropagatingHandler<ProbeHandler>>();
}
