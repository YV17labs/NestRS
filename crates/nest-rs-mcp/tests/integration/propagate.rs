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

use nest_rs_mcp::__private::{McpMount, endpoint};
use nest_rs_mcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams,
    CancelledNotificationParam, CompleteRequestParams, CompleteResult, ContentBlock,
    CustomNotification, CustomRequest, CustomResult, DiscoverResult, GetPromptRequestParams,
    GetPromptResponse, GetPromptResult, GetSkillRequestParams, GetSkillResult, GetTaskParams,
    GetTaskResult, ListPromptsResult, ListResourceTemplatesResult, ListResourcesResult,
    ListSkillsResult, ListToolsResult, PaginatedRequestParams, ProgressNotificationParam,
    PromptMessage, ProtocolVersion, ReadResourceDirectoryRequestParams,
    ReadResourceDirectoryResult, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, ResourceContents, Role, ServerCapabilities, ServerConfig,
    SetLevelRequestParams, SubscribeRequestParams, SubscriptionFilter, Tool,
    UnsubscribeRequestParams, UpdateTaskParams,
};
use nest_rs_mcp::rmcp::serde_json::{self, json};
use nest_rs_mcp::service::{NotificationContext, RequestContext, RoleServer};
use nest_rs_mcp::{McpError, PropagatingHandler, ServerHandler, mcp, tools};
use nest_rs_testing::TestClient;
use nest_rs_testing::mcp::{call_method, notify, open_session, open_session_with, result};
use nest_rs_testing::{LogCapture, TestApp};
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
                .enable_skills_with_directory_read()
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

    async fn read_resource_directory(
        &self,
        _request: ReadResourceDirectoryRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceDirectoryResult, McpError> {
        self.mark("read_resource_directory");
        Err(McpError::invalid_params("no directory", None))
    }

    async fn list_skills(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListSkillsResult, McpError> {
        self.mark("list_skills");
        Ok(ListSkillsResult::default())
    }

    async fn get_skill(
        &self,
        _request: GetSkillRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetSkillResult, McpError> {
        self.mark("get_skill");
        Err(McpError::invalid_params("no skill", None))
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
    "get_skill",
    "get_task",
    "get_tool",
    "list_prompts",
    "list_resource_templates",
    "list_resources",
    "list_skills",
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
    "read_resource_directory",
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
    let client = TestClient::new(endpoint(McpMount::public(), move || host.clone()));

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
        ("resources/directory/read", json!({ "uri": "probe://dir/" })),
        ("skills/list", json!({})),
        ("skills/get", json!({ "uri": "probe://skill/SKILL.md" })),
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

/// What a client named — a custom method, a tool no host declares, a prompt,
/// a resource, a task — is its own value: the `mcp.operation` line names the
/// protocol's method or `_OTHER`, and a tool only by the name a host declares,
/// so no line holds what the client wrote. The span keeps the conventions'
/// method and resource URI as sent, a stated exception, a tool or prompt as a
/// host declares it, `_OTHER` otherwise, as its name does, and no task id,
/// which no convention names; a notification has a span of its own.
#[tokio::test]
async fn an_operation_line_names_nothing_only_the_client_named() {
    let logs = LogCapture::install();
    let host = ProbeHandler {
        seen: Arc::default(),
    };
    let client = TestClient::new(endpoint(McpMount::public(), move || host.clone()));
    let session = open_session_with(&client, "/", None, &[], tasks_client_capabilities()).await;

    let requests: &[(&str, serde_json::Value)] = &[
        (
            "tools/call",
            json!({ "name": "marked-tool", "arguments": {} }),
        ),
        ("prompts/get", json!({ "name": "marked-prompt" })),
        ("resources/read", json!({ "uri": "marked://resource" })),
        ("tasks/get", json!({ "taskId": "marked-task" })),
        ("marked/custom", json!({})),
    ];
    for (method, params) in requests {
        call_method(&client, "/", &session, None, method, params.clone()).await;
    }
    notify(
        &client,
        "/",
        &session,
        None,
        "notifications/marked",
        json!({}),
    )
    .await;

    let lines = || {
        logs.find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_mcp::unit::OPERATION.name(),
        )
    };
    nest_rs_testing::wait_until(Duration::from_secs(2), || {
        lines()
            .iter()
            .filter(|line| {
                line.field("mcp_method").as_deref() == Some(nest_rs_core::operation_log::OTHER)
            })
            .count()
            == 2
    })
    .await;
    // rmcp's own events are its clamp's; the capture keeps them all.
    for event in logs
        .events()
        .into_iter()
        .filter(|event| event.target.starts_with("nest_rs"))
    {
        for (field, value) in &event.fields {
            assert!(
                !value.contains("marked"),
                "{}: {field} = {value}",
                event.target
            );
        }
    }
    let lines = lines();
    let line = |method: &str| {
        lines
            .iter()
            .find(|line| line.field("mcp_method").as_deref() == Some(method))
            .unwrap_or_else(|| panic!("a `{method}` line: {lines:#?}"))
    };
    assert_eq!(
        line("tools/call").field("tool").as_deref(),
        Some(nest_rs_core::operation_log::OTHER),
        "a tool no host declares"
    );
    assert_eq!(
        line("prompts/get").field("prompt").as_deref(),
        Some(nest_rs_core::operation_log::OTHER),
        "a prompt no host declares"
    );
    assert_eq!(
        line("resources/read").field("resource").as_deref(),
        Some(nest_rs_core::operation_log::OTHER),
        "a URI no template serves"
    );
    for field in ["tool", "prompt", "resource"] {
        assert_eq!(line("tasks/get").field(field), None, "{field}");
    }

    let span = |method: &str| {
        logs.spans()
            .into_iter()
            .find(|span| span.field("mcp.method.name").as_deref() == Some(method))
            .unwrap_or_else(|| panic!("a `{method}` span"))
    };
    let other = Some(nest_rs_core::operation_log::OTHER);
    let tool = span("tools/call");
    assert_eq!(tool.field("gen_ai.tool.name").as_deref(), other);
    assert_eq!(tool.field("otel.name").as_deref(), Some("tools/call"));
    let prompt = span("prompts/get");
    assert_eq!(prompt.field("gen_ai.prompt.name").as_deref(), other);
    assert_eq!(prompt.field("otel.name").as_deref(), Some("prompts/get"));
    assert_eq!(
        span("resources/read").field("mcp.resource.uri").as_deref(),
        Some("marked://resource"),
        "the conventions' resource URI, as sent"
    );
    for field in ["gen_ai.tool.name", "gen_ai.prompt.name", "mcp.resource.uri"] {
        assert_eq!(span("tasks/get").field(field), None, "{field}");
    }
    assert_eq!(
        span("marked/custom").field("otel.name").as_deref(),
        other,
        "a custom method names no span"
    );
    let notification = span("notifications/marked");
    assert_eq!(notification.field("otel.name").as_deref(), other);
    assert_eq!(notification.field("jsonrpc.request.id"), None);

    let operation = nest_rs_mcp::unit::OPERATION;
    let spans: Vec<_> = logs
        .spans()
        .into_iter()
        .filter(|span| span.target == operation.target() && span.name == operation.name())
        .collect();
    // A tool's name keys a tool call alone: each key is filled by some span.
    let left_empty_by_every_span = spans
        .iter()
        .map(|span| {
            span.duration_keys_left_empty(&operation)
                .expect("an operation's duration is recorded")
        })
        .reduce(|left, next| left.into_iter().filter(|key| next.contains(key)).collect())
        .expect("an operation's span");
    assert!(
        left_empty_by_every_span.is_empty(),
        "{left_empty_by_every_span:?}: {spans:#?}"
    );
}

const CONVENTIONS: &str = "/mcp/conventions";

#[mcp(path = "/mcp/conventions")]
#[derive(Clone, Default)]
struct WeatherHost;

#[tools]
impl WeatherHost {
    #[tool(description = "Tells the weather.")]
    #[public]
    async fn forecast(&self) -> Result<String, McpError> {
        Ok("sunny".to_owned())
    }

    /// Outline a forecast.
    #[prompt]
    #[public]
    async fn outlook(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            "outlook",
        )]))
    }

    /// One city's weather.
    #[resource(template = "weather://{city}")]
    #[public]
    async fn city(&self, city: String) -> Result<String, McpError> {
        Ok(format!("sunny in {city}"))
    }
}

#[module(imports = [crate::open_mcp()], providers = [WeatherHost])]
struct WeatherModule;

/// Each operation's span carries what OpenTelemetry's MCP conventions give its
/// kind, and is named `{method} {target}` for the tool or prompt it ran, the
/// method alone otherwise: a resource's URI is an attribute, never its name, a
/// tool call alone is `execute_tool`, and a notification, which carries no id,
/// is an operation of its own.
/// An operation's span names the protocol version it runs under: the one its
/// session negotiated, where its request names none.
#[tokio::test]
async fn an_operations_span_names_the_protocol_version_its_session_negotiated() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<WeatherModule>()
        .await
        .expect("the module boots");
    let session = open_session(app.http(), CONVENTIONS, None).await;
    call_method(
        app.http(),
        CONVENTIONS,
        &session,
        None,
        "tools/list",
        json!({}),
    )
    .await;

    let spans = logs.spans();
    let listing = spans
        .iter()
        .find(|span| span.field("mcp.method.name").as_deref() == Some("tools/list"))
        .unwrap_or_else(|| panic!("a `tools/list` span: {spans:#?}"));
    assert_eq!(
        listing.field("mcp.protocol.version").as_deref(),
        Some(nest_rs_testing::mcp::PROTOCOL_VERSION),
        "{:?}",
        listing.fields,
    );
}

#[tokio::test]
async fn an_operations_span_follows_the_mcp_conventions_for_its_kind() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<WeatherModule>()
        .await
        .expect("the module boots");
    let session = open_session(app.http(), CONVENTIONS, None).await;
    for (method, params) in [
        ("tools/call", json!({ "name": "forecast", "arguments": {} })),
        ("prompts/get", json!({ "name": "outlook" })),
        ("resources/read", json!({ "uri": "weather://paris" })),
        ("tools/list", json!({})),
    ] {
        call_method(app.http(), CONVENTIONS, &session, None, method, params).await;
    }

    let spans = logs.spans();
    let span = |method: &str| {
        spans
            .iter()
            .find(|span| span.field("mcp.method.name").as_deref() == Some(method))
            .unwrap_or_else(|| panic!("a `{method}` span: {spans:#?}"))
    };
    let expected = [
        (
            "tools/call",
            "tools/call forecast",
            [Some("forecast"), None, Some("execute_tool"), None],
        ),
        (
            "prompts/get",
            "prompts/get outlook",
            [None, Some("outlook"), None, None],
        ),
        (
            "resources/read",
            "resources/read",
            [None, None, None, Some("weather://paris")],
        ),
        ("tools/list", "tools/list", [None; 4]),
    ];
    for (method, name, addressed) in expected {
        let span = span(method);
        assert_eq!(span.field("otel.name").as_deref(), Some(name), "{method}");
        let fields = [
            "gen_ai.tool.name",
            "gen_ai.prompt.name",
            "gen_ai.operation.name",
            "mcp.resource.uri",
        ];
        for (field, value) in fields.into_iter().zip(addressed) {
            assert_eq!(span.field(field).as_deref(), value, "{method}: {field}");
        }
        assert_eq!(
            span.field("jsonrpc.request.id").as_deref(),
            Some("99"),
            "{method}"
        );
        assert_eq!(span.field("network.transport").as_deref(), Some("tcp"));
        assert_eq!(span.field("network.protocol.name").as_deref(), Some("http"));
    }

    let initialized = span("notifications/initialized");
    assert_eq!(
        initialized.field("otel.name").as_deref(),
        Some("notifications/initialized")
    );
    assert_eq!(initialized.field("jsonrpc.request.id"), None);
    assert_ne!(
        initialized.field("span_id"),
        initialized.field("parent_span_id"),
        "a notification is an operation of its own, under the request carrying it",
    );
}

const REFUSALS: &str = "/mcp/refusals";

#[mcp(path = "/mcp/refusals")]
#[derive(Clone, Default)]
struct DraftPrompts;

#[tools]
impl DraftPrompts {
    /// Draft a note.
    #[prompt]
    #[public]
    async fn draft(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            "draft",
        )]))
    }
}

#[module(imports = [crate::open_mcp()], providers = [DraftPrompts])]
struct DraftModule;

/// A prompt's line names it as its `mounted` line does, and `_OTHER` for a name
/// no host declares.
#[tokio::test]
async fn a_prompt_line_names_what_the_mount_declares() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<DraftModule>()
        .await
        .expect("a prompt host boots");
    let session = open_session(app.http(), REFUSALS, None).await;
    for name in ["draft", "marked-prompt"] {
        call_method(
            app.http(),
            REFUSALS,
            &session,
            None,
            "prompts/get",
            json!({ "name": name }),
        )
        .await;
    }

    let named: Vec<_> = logs
        .find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_mcp::unit::OPERATION.name(),
        )
        .into_iter()
        .filter(|line| line.field("mcp_method").as_deref() == Some("prompts/get"))
        .map(|line| line.field("prompt"))
        .collect();
    assert_eq!(
        named,
        [
            Some("draft".to_owned()),
            Some(nest_rs_core::operation_log::OTHER.to_owned()),
        ],
    );
    let mounted = logs.expect_one(nest_rs_http::target::ROUTES, "mounted");
    assert_eq!(mounted.field("prompt").as_deref(), Some("draft"));
}

/// rmcp refuses a prompt its host does not declare, and a method no host
/// serves, in words quoting the name a client sent: each is answered by
/// kind, the prompt's refusal still listing the prompts the host declares.
#[tokio::test]
async fn a_refusal_of_a_name_no_host_serves_quotes_none_of_it() {
    let app = TestApp::for_module::<DraftModule>()
        .await
        .expect("a prompt host boots");
    let session = open_session(app.http(), REFUSALS, None).await;
    let call = |method: &'static str, params: serde_json::Value| {
        call_method(app.http(), REFUSALS, &session, None, method, params)
    };
    let prompt = call("prompts/get", json!({ "name": "marked-prompt" })).await;
    let custom = call("marked/custom", json!({})).await;

    for body in [&prompt, &custom] {
        assert!(!body.contains("marked"), "{body}");
    }
    let prompt = result(&prompt);
    assert_eq!(prompt["error"]["code"], json!(-32602), "{prompt}");
    assert_eq!(
        prompt["error"]["data"]["available_prompts"],
        json!(["draft"]),
        "{prompt}"
    );
    let custom = result(&custom);
    assert_eq!(custom["error"]["code"], json!(-32601), "{custom}");
}

/// Every method no host serves answers `-32601` under the `NOT_FOUND` class,
/// whoever wrote the refusal: the endpoint for a custom method, a host's rmcp
/// default for a protocol method it does not serve, both in band, and rmcp's
/// dispatch for a method the request's version does not have, answered `404`.
#[tokio::test]
async fn a_method_no_host_serves_names_the_not_found_class() {
    let app = TestApp::for_module::<DraftModule>()
        .await
        .expect("a prompt host boots");
    let session = open_session(app.http(), REFUSALS, None).await;
    for (method, params) in [
        ("marked/custom", json!({})),
        ("resources/read", json!({ "uri": "file:///draft" })),
        ("logging/setLevel", json!({ "level": "info" })),
    ] {
        let body = call_method(app.http(), REFUSALS, &session, None, method, params).await;
        let refused = result(&body);
        assert_eq!(
            refused["error"]["code"],
            json!(-32601),
            "{method}: {refused}"
        );
        assert_eq!(
            refused["error"]["data"],
            json!({ "code": "NOT_FOUND" }),
            "{method}: {refused}"
        );
    }

    let unversioned = nest_rs_testing::mcp::post_message_with(
        app.http(),
        REFUSALS,
        None,
        None,
        &[
            ("mcp-protocol-version", MODERN_VERSION),
            ("mcp-method", "ping"),
        ],
        &json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "ping",
            "params": { "_meta": modern_meta() },
        }),
    )
    .await;
    assert_eq!(unversioned.0.status(), nest_rs_http::StatusCode::NOT_FOUND);
    let body = unversioned
        .0
        .into_body()
        .into_string()
        .await
        .expect("a JSON-RPC error");
    let refused = result(&body);
    assert_eq!(refused["error"]["code"], json!(-32601), "{refused}");
    assert_eq!(
        refused["error"]["data"],
        json!({ "code": "NOT_FOUND" }),
        "{refused}"
    );
}

/// A path one host serves.
const CLASS_LONE: &str = "/mcp/class/lone";

/// A path two hosts share, the mount reading what each declares.
const CLASS_SHARED: &str = "/mcp/class/shared";

/// A path two hosts share, one writing rmcp's handler by hand: the mount
/// cannot read what it declares, so only its own handler says, and it holds
/// tasks.
const CLASS_UNREAD: &str = "/mcp/class/unread";

/// The shapes of path a refusal reaches a client through.
const CLASS_PATHS: [&str; 3] = [CLASS_LONE, CLASS_SHARED, CLASS_UNREAD];

/// The versions a client speaks: today's two, each opening a session, and
/// rmcp's latest, which carries its lifecycle in every request.
const SPOKEN: [&str; 3] = ["2025-06-18", "2025-11-25", MODERN_VERSION];

#[mcp(path = "/mcp/class/lone")]
#[derive(Clone, Default)]
struct LoneClass;

#[tools]
impl LoneClass {
    #[tool(description = "Answers at once.")]
    #[public]
    async fn lone_ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }

    /// Open a note.
    #[prompt]
    #[public]
    async fn lone_prompt(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            "lone",
        )]))
    }
}

#[mcp(path = "/mcp/class/shared")]
#[derive(Clone, Default)]
struct SharedClassFirst;

#[tools]
impl SharedClassFirst {
    #[tool(description = "Answers at once.")]
    #[public]
    async fn first_ping(&self) -> Result<String, McpError> {
        Ok("first".to_owned())
    }

    /// Open a note.
    #[prompt]
    #[public]
    async fn first_prompt(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            "first",
        )]))
    }
}

#[mcp(path = "/mcp/class/shared")]
#[derive(Clone, Default)]
struct SharedClassSecond;

#[tools]
impl SharedClassSecond {
    #[tool(description = "Answers at once.")]
    #[public]
    async fn second_ping(&self) -> Result<String, McpError> {
        Ok("second".to_owned())
    }
}

#[mcp(path = "/mcp/class/unread")]
#[derive(Clone, Default)]
struct ReadClass;

#[tools]
impl ReadClass {
    #[tool(description = "Answers at once.")]
    #[public]
    async fn read_ping(&self) -> Result<String, McpError> {
        Ok("read".to_owned())
    }
}

task_host!(UnreadClass, "/mcp/class/unread", "start_unread");

#[module(
    imports = [crate::open_mcp()],
    providers = [LoneClass, SharedClassFirst, SharedClassSecond, ReadClass, UnreadClass],
)]
struct ClassModule;

/// A client of one path, speaking one protocol version.
struct Speaking<'a> {
    app: &'a TestApp,
    path: &'static str,
    version: &'static str,
    /// `None` from `2026-07-28`, whose requests open none.
    session: Option<String>,
}

impl<'a> Speaking<'a> {
    /// A client of `path` at `version`: below `2026-07-28` a session
    /// `initialize` negotiated at that version, declaring the tasks extension.
    async fn open(app: &'a TestApp, path: &'static str, version: &'static str) -> Self {
        let session = if version < MODERN_VERSION {
            let mut handshake =
                nest_rs_testing::mcp::initialize_request_with(tasks_client_capabilities());
            handshake["params"]["protocolVersion"] = json!(version);
            let init =
                nest_rs_testing::mcp::post_message(app.http(), path, None, None, &handshake).await;
            init.assert_status_is_ok();
            let session = init
                .0
                .headers()
                .get("mcp-session-id")
                .and_then(|value| value.to_str().ok())
                .expect("initialize opens a session")
                .to_owned();
            let body = init.0.into_body().into_string().await.expect("a body");
            assert_eq!(
                result(&body)["result"]["protocolVersion"],
                json!(version),
                "{path}: the session speaks the version asked for",
            );
            nest_rs_testing::mcp::post_message_with(
                app.http(),
                path,
                Some(&session),
                None,
                &[("mcp-protocol-version", version)],
                &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            )
            .await;
            Some(session)
        } else {
            None
        };
        Self {
            app,
            path,
            version,
            session,
        }
    }

    /// Send `method` with `params` as a client at this version does, and
    /// return the raw response body: from `2026-07-28` with the standard
    /// headers and the lifecycle in `_meta`.
    async fn send(&self, method: &str, mut params: serde_json::Value) -> String {
        let named = ["name", "uri", "taskId"]
            .into_iter()
            .find_map(|key| params.get(key).and_then(serde_json::Value::as_str))
            .map(str::to_owned);
        let mut headers = vec![("mcp-protocol-version", self.version)];
        if self.session.is_none() {
            headers.push(("mcp-method", method));
            if let Some(named) = &named {
                headers.push(("mcp-name", named));
            }
            params["_meta"] = modern_meta();
        }
        nest_rs_testing::mcp::post_message_with(
            self.app.http(),
            self.path,
            self.session.as_deref(),
            None,
            &headers,
            &json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params }),
        )
        .await
        .0
        .into_body()
        .into_string()
        .await
        .expect("a JSON-RPC response body")
    }
}

/// The `mcp.operation` lines a `tools/call` filed, in order.
fn tool_call_lines(logs: &LogCapture) -> Vec<nest_rs_testing::CapturedEvent> {
    logs.find(
        nest_rs_core::operation_log::TARGET,
        nest_rs_mcp::unit::OPERATION.name(),
    )
    .into_iter()
    .filter(|line| line.field("mcp_method").as_deref() == Some("tools/call"))
    .collect()
}

/// A tool no host declares is refused under the `NOT_FOUND` class at every
/// version a client speaks, by rmcp's handler at each, on a path one host
/// serves, one several share, and one whose host the mount cannot read. rmcp's code and message stay,
/// the name sent is in no reply, and the call files one line.
#[tokio::test]
async fn a_tool_no_host_declares_names_the_not_found_class_at_every_version() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<ClassModule>()
        .await
        .expect("three paths of hosts boot");
    for path in CLASS_PATHS {
        for version in SPOKEN {
            let client = Speaking::open(&app, path, version).await;
            let filed = tool_call_lines(&logs).len();

            let body = client
                .send(
                    "tools/call",
                    json!({ "name": "marked_tool", "arguments": {} }),
                )
                .await;

            assert!(!body.contains("marked"), "{path} at {version}: {body}");
            let refused = result(&body);
            assert_eq!(
                refused["error"]["code"],
                json!(-32602),
                "{path} at {version}: {refused}"
            );
            assert_eq!(
                refused["error"]["message"],
                json!("tool not found"),
                "{path} at {version}: {refused}"
            );
            assert_eq!(
                refused["error"]["data"],
                json!({ "code": "NOT_FOUND" }),
                "{path} at {version}: {refused}"
            );
            let lines = tool_call_lines(&logs);
            let [line] = &lines[filed..] else {
                panic!("{path} at {version}: one line for the call: {lines:#?}");
            };
            assert_eq!(
                line.field("outcome").as_deref(),
                Some(nest_rs_core::operation_log::ERROR),
                "{path} at {version}",
            );
            assert_eq!(
                line.field("tool").as_deref(),
                Some(nest_rs_core::operation_log::OTHER),
                "{path} at {version}",
            );
        }
    }
}

/// Every other address no host serves is refused under the `NOT_FOUND` class
/// at every version a client speaks, on each shape of path: a prompt, a
/// custom method, a protocol method no host serves, and a task on the path
/// whose host holds tasks — never quoting what the client named.
#[tokio::test]
async fn every_address_no_host_serves_names_the_not_found_class_at_every_version() {
    let app = TestApp::for_module::<ClassModule>()
        .await
        .expect("three paths of hosts boot");
    for path in CLASS_PATHS {
        for version in SPOKEN {
            let client = Speaking::open(&app, path, version).await;
            let mut addressed = vec![
                ("prompts/get", json!({ "name": "marked-prompt" }), -32602),
                ("marked/custom", json!({}), -32601),
                ("resources/read", json!({ "uri": "file:///marked" }), -32601),
            ];
            if path == CLASS_UNREAD {
                addressed.push(("tasks/get", json!({ "taskId": "marked-task" }), -32602));
            }
            for (method, params, code) in addressed {
                let body = client.send(method, params).await;

                assert!(
                    !body.contains("marked"),
                    "{path} at {version}, {method}: {body}"
                );
                let refused = result(&body);
                assert_eq!(
                    refused["error"]["code"],
                    json!(code),
                    "{path} at {version}, {method}: {refused}"
                );
                assert_eq!(
                    refused["error"]["data"]["code"],
                    json!("NOT_FOUND"),
                    "{path} at {version}, {method}: {refused}"
                );
            }
        }
    }
}

/// POST one stateless `2026-07-28` request: no session id, the negotiated
/// version in the header, and whatever SEP-2243 standard headers the case needs.
async fn post_modern(
    client: &TestClient,
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

#[module(imports = [crate::open_mcp()], providers = [StoppingTools])]
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
            .filter(|line| line.field("tool").as_deref() == Some(operation))
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
    let (mut open, head) = crate::post(
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
    let mut read = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio::io::AsyncReadExt::read_to_end(&mut open, &mut read),
    )
    .await
    .expect("the transport closed the call's stream")
    .expect("the stream reads to its end");
    let read = String::from_utf8_lossy(&read);
    assert!(
        !read.contains("jsonrpc"),
        "the client reads no answer to an operation the transport stopped: {read}",
    );

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
    let read = tokio::time::timeout(Duration::from_secs(5), call)
        .await
        .expect("rmcp ends the stream of a request its client cancelled");
    assert!(
        !read.contains("jsonrpc"),
        "rmcp drops the answer to a request its client cancelled: {read}",
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

#[module(imports = [crate::open_mcp()], providers = [ExplodingTools])]
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
    assert_eq!(
        answer["error"]["data"],
        json!({ "code": nest_rs_core::code::INTERNAL.as_str() }),
        "the class every edge names an opaque failure by: {answer}",
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
                && span.field("gen_ai.tool.name").as_deref() == Some("boom")
        })
        .expect("the operation's span");
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::PANIC),
        "{:?}",
        span.fields,
    );
    assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
    assert_eq!(
        span.field("rpc.response.status_code").as_deref(),
        Some("-32603"),
        "the code its client is answered with: {:?}",
        span.fields,
    );
    let contained = logs.expect_one(
        nest_rs_mcp::TARGET,
        "mcp operation panicked; its client is answered with an internal error",
    );
    assert_eq!(contained.level, "error");
    assert_eq!(contained.field("tool").as_deref(), Some("boom"));
    assert!(
        contained
            .field(nest_rs_core::panic::FIELD)
            .is_some_and(|panic| panic.contains("the tool exploded")),
        "the operator reads what unwound: {contained:#?}",
    );
}

const REPORTING: &str = "/mcp/reporting";

#[nest_rs_core::input]
struct AmountArgs {
    amount: u64,
}

#[mcp(path = "/mcp/reporting")]
#[derive(Clone, Default)]
struct ReportingTools;

#[tools]
impl ReportingTools {
    #[tool(description = "Echoes an amount.")]
    #[public]
    async fn amount(
        &self,
        nest_rs_mcp::Parameters(args): nest_rs_mcp::Parameters<AmountArgs>,
    ) -> Result<String, McpError> {
        Ok(args.amount.to_string())
    }

    #[tool(description = "Reports its own error in its result.")]
    #[public]
    async fn reported(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::error(vec![ContentBlock::text(
            "the tool failed",
        )]))
    }
}

#[module(imports = [crate::open_mcp()], providers = [ReportingTools])]
struct ReportingModule;

/// A tool's error in band — rmcp's answer to arguments that do not decode, or
/// the tool's own `CallToolResult::error` — fails its unit, which rolls back:
/// the line says what the commit decided, and the span records the class the
/// MCP semantic conventions name for it, `tool_error`. The client's answer is
/// the tool result it was.
#[tokio::test]
async fn a_tool_error_in_band_files_its_line_error_and_its_span_tool_error() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<ReportingModule>()
        .await
        .expect("the module boots");
    let session = open_session(app.http(), REPORTING, None).await;

    for (tool, arguments) in [
        ("amount", json!({ "amount": "sk_live_51HsecretTOKEN" })),
        ("reported", json!({})),
    ] {
        let body = call_method(
            app.http(),
            REPORTING,
            &session,
            None,
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        )
        .await;
        let answer = result(&body);
        assert_eq!(answer["result"]["isError"], json!(true), "{tool}: {answer}");

        let line = operation_line(&logs, tool).await;
        assert_eq!(
            line.field("outcome").as_deref(),
            Some(nest_rs_core::operation_log::ERROR),
            "{tool}: the unit rolled back, and its line says so",
        );
        let span = logs
            .spans()
            .into_iter()
            .find(|span| {
                span.name == nest_rs_mcp::unit::OPERATION.name()
                    && span.field("gen_ai.tool.name").as_deref() == Some(tool)
            })
            .unwrap_or_else(|| panic!("{tool}: the operation's span"));
        assert_eq!(
            span.field("error.type").as_deref(),
            Some("tool_error"),
            "{tool}: {:?}",
            span.fields,
        );
        assert_eq!(span.field("otel.status_code").as_deref(), Some("error"));
    }

    call_method(
        app.http(),
        REPORTING,
        &session,
        None,
        "tools/call",
        json!({ "name": "amount", "arguments": { "amount": 7 } }),
    )
    .await;
    let answered = || {
        logs.find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_mcp::unit::OPERATION.name(),
        )
        .into_iter()
        .filter(|line| line.field("tool").as_deref() == Some("amount"))
        .nth(1)
    };
    nest_rs_testing::wait_until(Duration::from_secs(2), || answered().is_some()).await;
    assert_eq!(
        answered().and_then(|line| line.field("outcome")).as_deref(),
        Some(nest_rs_core::operation_log::OK),
        "a call its tool answered stays ok",
    );
}

const FAILING: &str = "/mcp/failing";

#[mcp(path = "/mcp/failing")]
#[derive(Clone, Default)]
struct FailingTools;

#[tools]
impl FailingTools {
    #[tool(description = "Fails in a way its client is not told of.")]
    #[public]
    async fn broken(&self) -> Result<String, std::io::Error> {
        Err(std::io::Error::other("the ledger at 10.0.0.1 refused"))
    }

    #[tool(description = "Refuses its call as a conflict.")]
    #[public]
    async fn taken(&self) -> Result<String, nest_rs_core::Problem> {
        Err(nest_rs_core::Problem::new(
            409,
            nest_rs_core::code::CONFLICT,
        ))
    }

    /// Draft a note.
    #[prompt]
    #[public]
    async fn draft(&self) -> Result<GetPromptResult, McpError> {
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            "draft",
        )]))
    }
}

#[module(imports = [crate::open_mcp()], providers = [FailingTools])]
struct FailingModule;

/// An operation answered with a JSON-RPC error files its line `error`, its
/// unit rolled back, and its span records the code as OpenTelemetry's MCP
/// conventions ask: `rpc.response.status_code` always, and `error.type` with
/// an `Error` status only for a code they read as the server's failure — a
/// code they read as the caller's own (`-32600`, `-32601`, `-32602`) fails
/// nothing on the server, as an HTTP `4xx` does not.
#[tokio::test]
async fn a_json_rpc_error_records_its_code_on_the_operations_span() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<FailingModule>()
        .await
        .expect("the module boots");
    let session = open_session(app.http(), FAILING, None).await;
    let call = |method: &'static str, params: serde_json::Value| {
        call_method(app.http(), FAILING, &session, None, method, params)
    };
    let other = nest_rs_core::operation_log::OTHER;
    let answered = [
        (
            "tools/call",
            ("gen_ai.tool.name", "broken"),
            json!({ "name": "broken", "arguments": {} }),
            -32603,
            true,
        ),
        (
            "tools/call",
            ("gen_ai.tool.name", "taken"),
            json!({ "name": "taken", "arguments": {} }),
            -32600,
            false,
        ),
        (
            "tools/call",
            ("gen_ai.tool.name", other),
            json!({ "name": "missing", "arguments": {} }),
            -32602,
            false,
        ),
        (
            "prompts/get",
            ("gen_ai.prompt.name", other),
            json!({ "name": "missing" }),
            -32602,
            false,
        ),
        (
            "resources/read",
            ("mcp.resource.uri", "missing://resource"),
            json!({ "uri": "missing://resource" }),
            -32601,
            false,
        ),
    ];
    let calls = answered.len();
    for (method, (field, addressed), params, code, fails) in answered {
        let body = call(method, params).await;
        let answer = result(&body);
        assert_eq!(answer["error"]["code"], json!(code), "{method}: {answer}");

        let said = code.to_string();
        let span = || {
            logs.spans().into_iter().find(|span| {
                span.name == nest_rs_mcp::unit::OPERATION.name()
                    && span.field("mcp.method.name").as_deref() == Some(method)
                    && span.field(field).as_deref() == Some(addressed)
                    && span.field("rpc.response.status_code").is_some()
            })
        };
        nest_rs_testing::wait_until(Duration::from_secs(2), || span().is_some()).await;
        let span = span().unwrap_or_else(|| panic!("{method} {addressed}: its span"));
        assert_eq!(
            span.field("rpc.response.status_code").as_deref(),
            Some(said.as_str()),
            "{method} {addressed}: {:?}",
            span.fields,
        );
        let (error_type, status) = if fails {
            (Some(said.as_str()), Some("error"))
        } else {
            (None, None)
        };
        assert_eq!(
            span.field("error.type").as_deref(),
            error_type,
            "{method} {addressed}: {:?}",
            span.fields,
        );
        assert_eq!(
            span.field("otel.status_code").as_deref(),
            status,
            "{method} {addressed}: {:?}",
            span.fields,
        );
    }
    let lines = logs.find(
        nest_rs_core::operation_log::TARGET,
        nest_rs_mcp::unit::OPERATION.name(),
    );
    let failed: Vec<_> = lines
        .iter()
        .filter(|line| {
            matches!(
                line.field("mcp_method").as_deref(),
                Some("tools/call" | "prompts/get" | "resources/read")
            )
        })
        .map(|line| line.field("outcome"))
        .collect();
    assert_eq!(
        failed,
        vec![Some(nest_rs_core::operation_log::ERROR.to_owned()); calls],
        "every unit rolled back, and its line says so: {lines:#?}",
    );
}

const SUBSCRIBED: &str = "/mcp/subscribed";

/// A host whose subscription only its client ends: `listen` waits for the
/// cancellation and nothing else, as rmcp's own default does.
#[mcp(path = "/mcp/subscribed")]
#[derive(Clone)]
struct SubscribedHost;

#[nest_rs_mcp::rmcp::tool_router(allow_empty)]
impl SubscribedHost {}

#[nest_rs_mcp::rmcp::tool_handler]
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

#[module(imports = [crate::open_mcp()], providers = [SubscribedHost])]
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
        .filter(|line| line.field("mcp_method").as_deref() == Some("subscriptions/listen"))
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

const STALLED: &str = "/mcp/stalled";

/// Set when `stalled`'s future is dropped, and once it has started.
static STALLED_DROPPED: AtomicBool = AtomicBool::new(false);
static STALLED_STARTED: Notify = Notify::const_new();

#[mcp(path = "/mcp/stalled")]
#[derive(Clone, Default)]
struct StalledTools;

#[tools]
impl StalledTools {
    #[tool(description = "Never answers.")]
    #[public]
    async fn stalled(&self) -> Result<String, McpError> {
        let _dropped = SetOnDrop(&STALLED_DROPPED);
        STALLED_STARTED.notify_one();
        std::future::pending::<()>().await;
        Ok("never".to_owned())
    }
}

/// sse-stream re-arms rmcp's keep-alive on the system clock, which a paused
/// tokio clock runs past: every poll would find it due and flood the stream
/// ahead of the answer.
fn without_keep_alive() -> nest_rs_mcp::McpConfig {
    nest_rs_mcp::McpConfig {
        sse_keep_alive: None,
        ..nest_rs_mcp::McpConfig::default()
    }
}

/// `config`, the endpoint opened in code, so the tests reach their tools.
fn opened(config: nest_rs_mcp::McpConfig) -> (nest_rs_mcp::McpAccess, nest_rs_mcp::McpConfig) {
    (nest_rs_mcp::McpAccess::Public, config)
}

#[module(
    imports = [nest_rs_mcp::McpModule::for_root(opened(without_keep_alive()))],
    providers = [StalledTools],
)]
struct StalledModule;

/// The HTTP request timeout the stalled tool is served under: the floor.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

/// What the operation deadline answers past `McpConfig::operation_timeout`'s
/// default, 20 seconds.
const PAST_THE_OPERATION_DEADLINE: Duration = Duration::from_secs(21);

async fn serve_stalled<M: nest_rs_core::Module + 'static>() -> crate::Serving {
    crate::serve_under::<M>(nest_rs_http::HttpConfig {
        request_timeout: Some(REQUEST_TIMEOUT),
        ..nest_rs_http::HttpConfig::default()
    })
    .await
}

/// Reads the response stream until the JSON-RPC message answering `id` arrives.
async fn answer_to(stream: &mut tokio::net::TcpStream, id: u64) -> serde_json::Value {
    use tokio::io::AsyncReadExt as _;
    let mut read = Vec::new();
    let mut chunk = [0_u8; 4096];
    let found = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let text = String::from_utf8_lossy(&read).into_owned();
            if let Some(message) = text
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
                .filter_map(|data| serde_json::from_str::<serde_json::Value>(data).ok())
                .find(|message| message["id"] == id)
            {
                return message;
            }
            let n = stream
                .read(&mut chunk)
                .await
                .expect("the stream is readable");
            assert!(n > 0, "the stream ended unanswered: {text:?}");
            read.extend_from_slice(&chunk[..n]);
        }
    })
    .await;
    found.unwrap_or_else(|_| {
        panic!(
            "no answer to {id}: {:?}",
            String::from_utf8_lossy_owned(read)
        )
    })
}

/// The HTTP request timeout bounds a response head, and a `tools/call` answers
/// its head at once with an event stream: the operation deadline bounds it.
async fn assert_cut_at_the_operation_deadline(
    logs: &LogCapture,
    stream: &mut tokio::net::TcpStream,
) {
    tokio::time::timeout(Duration::from_secs(5), STALLED_STARTED.notified())
        .await
        .expect("the tool starts");
    tokio::time::pause();
    tokio::time::sleep(REQUEST_TIMEOUT * 2).await;
    assert!(
        !STALLED_DROPPED.load(Ordering::SeqCst),
        "the request timeout does not reach an operation rmcp runs on its own task",
    );
    tokio::time::sleep(PAST_THE_OPERATION_DEADLINE - REQUEST_TIMEOUT * 2).await;
    tokio::time::resume();

    let answer = answer_to(stream, 7).await;
    assert_eq!(
        answer["error"]["code"], -32603,
        "an internal error: {answer}"
    );
    assert_eq!(
        answer["error"]["message"], "the operation was not answered in time",
        "{answer}"
    );
    assert_eq!(answer["error"]["data"]["reason"], "timeout", "{answer}");
    assert_eq!(answer["error"]["data"]["code"], "UNAVAILABLE", "{answer}");
    assert!(
        STALLED_DROPPED.load(Ordering::SeqCst),
        "the operation was dropped at its deadline"
    );
    let line = operation_line(logs, "stalled").await;
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::ERROR),
    );
    let cut = logs.expect_one(
        nest_rs_mcp::TARGET,
        "mcp operation not answered within its deadline; it is cut",
    );
    assert_eq!(cut.level, "warn");
    assert_eq!(cut.field("reason").as_deref(), Some("timeout"));
    assert_eq!(cut.field("tool").as_deref(), Some("stalled"));
    assert_eq!(cut.field("timeout_ms").as_deref(), Some("20000"));
}

#[tokio::test]
async fn a_session_tool_call_past_the_request_timeout_is_cut_at_the_operation_deadline() {
    let logs = LogCapture::install();
    let serving = serve_stalled::<StalledModule>().await;
    let session = crate::open_raw_session(serving.port, STALLED).await;
    let (mut stream, head) = crate::post(
        serving.port,
        STALLED,
        Some(&session),
        &json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "stalled", "arguments": {} }
        }),
    )
    .await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "the call is accepted: {head}"
    );

    assert_cut_at_the_operation_deadline(&logs, &mut stream).await;
    serving.stop().await;
}

#[module(
    imports = [nest_rs_mcp::McpModule::for_root(opened(nest_rs_mcp::McpConfig {
        legacy_session_mode: false,
        ..without_keep_alive()
    }))],
    providers = [StalledTools],
)]
struct StatelessStalledModule;

/// A legacy client with sessions turned off is served statelessly, its answer
/// streamed after a head sent at once.
#[tokio::test]
async fn a_stateless_tool_call_past_the_request_timeout_is_cut_at_the_operation_deadline() {
    let logs = LogCapture::install();
    let serving = serve_stalled::<StatelessStalledModule>().await;
    let (mut stream, head) = crate::post(
        serving.port,
        STALLED,
        None,
        &json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "stalled", "arguments": {} }
        }),
    )
    .await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "the call is accepted: {head}"
    );

    assert_cut_at_the_operation_deadline(&logs, &mut stream).await;
    serving.stop().await;
}

/// A `2026-07-28` request holds its head until the operation's first message,
/// so the request timeout reaches it first, and rmcp drops the operation with
/// the response it can no longer send.
#[tokio::test]
async fn a_modern_tool_call_is_cut_by_the_request_timeout_before_it_answers() {
    let logs = LogCapture::install();
    let serving = serve_stalled::<StalledModule>().await;
    let body = json!({
        "jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": { "name": "stalled", "arguments": {}, "_meta": modern_meta() },
    })
    .to_string();
    let request = format!(
        "POST {STALLED} HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n\
         accept: application/json, text/event-stream\r\nmcp-protocol-version: {MODERN_VERSION}\r\n\
         mcp-method: tools/call\r\nmcp-name: stalled\r\ncontent-length: {}\r\n\r\n{body}",
        body.len(),
    );
    let port = serving.port;
    let exchanged = tokio::spawn(async move { crate::exchange(port, &request).await });
    tokio::time::timeout(Duration::from_secs(5), STALLED_STARTED.notified())
        .await
        .expect("the tool starts");
    tokio::time::pause();
    tokio::time::sleep(REQUEST_TIMEOUT * 2).await;
    tokio::time::resume();

    let (_stream, head) = exchanged.await.expect("the exchange does not panic");
    assert!(
        head.starts_with("HTTP/1.1 503"),
        "the request timeout answered: {head}"
    );
    let line = operation_line(&logs, "stalled").await;
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
    );
    assert!(
        STALLED_DROPPED.load(Ordering::SeqCst),
        "the operation was dropped"
    );
    serving.stop().await;
}

/// How a unit the recording binding opened ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnitEnd {
    /// Settled, told whether the unit succeeded.
    Settled { ok: bool },
    /// Dropped before it settled: whatever it held stays unsettled.
    Abandoned,
}

static UNITS_ENDED: Mutex<Vec<UnitEnd>> = Mutex::new(Vec::new());

fn units_ended() -> Vec<UnitEnd> {
    UNITS_ENDED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// A unit-of-work binding recording how each unit it opened ended.
#[nest_rs_core::injectable]
#[derive(Default)]
struct RecordingUnitOfWork;

/// One unit of [`RecordingUnitOfWork`], which records its end once.
struct RecordingUnit {
    settled: bool,
}

fn record(end: UnitEnd) {
    UNITS_ENDED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(end);
}

struct NoExecutor;

impl nest_rs_database::Executor for NoExecutor {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl nest_rs_database::UnitOfWork for RecordingUnitOfWork {
    fn open(
        &self,
        _: &nest_rs_database::UnitSpec,
    ) -> Result<Box<dyn nest_rs_database::OpenUnit>, nest_rs_database::Unhonoured> {
        Ok(Box::new(RecordingUnit { settled: false }))
    }

    fn settle_net(&self) -> Duration {
        Duration::from_secs(16)
    }
}

impl nest_rs_database::OpenUnit for RecordingUnit {
    fn executor(&self) -> Arc<dyn nest_rs_database::Executor> {
        Arc::new(NoExecutor)
    }

    fn settle(
        mut self: Box<Self>,
        ok: bool,
    ) -> std::pin::Pin<Box<dyn Future<Output = nest_rs_database::Settlement> + Send>> {
        self.settled = true;
        record(UnitEnd::Settled { ok });
        Box::pin(std::future::ready(nest_rs_database::Settlement::Settled))
    }
}

impl Drop for RecordingUnit {
    fn drop(&mut self) {
        if !self.settled {
            record(UnitEnd::Abandoned);
        }
    }
}

#[module(
    imports = [nest_rs_mcp::McpModule::for_root(opened(without_keep_alive()))],
    providers = [
        StalledTools,
        RecordingUnitOfWork as dyn nest_rs_database::UnitOfWork],
)]
struct StalledInUnitModule;

/// The deadline cuts the operation's body inside its unit, never the unit
/// around it: one cut around the unit would drop it unsettled, a transaction
/// neither committed nor rolled back.
#[tokio::test]
async fn a_tool_call_cut_at_its_deadline_settles_its_unit_as_a_failure() {
    let logs = LogCapture::install();
    let serving = serve_stalled::<StalledInUnitModule>().await;
    let session = crate::open_raw_session(serving.port, STALLED).await;
    let (mut stream, head) = crate::post(
        serving.port,
        STALLED,
        Some(&session),
        &json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "stalled", "arguments": {} }
        }),
    )
    .await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "the call is accepted: {head}"
    );

    assert_cut_at_the_operation_deadline(&logs, &mut stream).await;
    let ended = units_ended();
    assert!(
        !ended.contains(&UnitEnd::Abandoned),
        "no unit was dropped unsettled: {ended:?}",
    );
    assert_eq!(
        ended
            .iter()
            .filter(|end| **end == UnitEnd::Settled { ok: false })
            .count(),
        1,
        "the cut call's unit settled as a failure: {ended:?}",
    );
    serving.stop().await;
}

const SETTLING: &str = "/mcp/settling";

#[mcp(path = "/mcp/settling")]
#[derive(Clone, Default)]
struct SettlingTools;

#[tools]
impl SettlingTools {
    #[tool(description = "Answers at once; its unit takes its time to commit.")]
    #[public]
    async fn quick(&self) -> Result<String, McpError> {
        Ok("committed".to_owned())
    }
}

/// Told when a unit's settle has started, and told back to let it land.
static SETTLE_STARTED: Notify = Notify::const_new();
static SETTLE_RELEASED: Notify = Notify::const_new();
/// Set once a held commit landed.
static SETTLE_LANDED: AtomicBool = AtomicBool::new(false);
/// Whether the next operation to settle is held — the tool call's, not the
/// handshake's.
static HOLD_NEXT: AtomicBool = AtomicBool::new(false);

/// A binding whose commit waits, on a task of its own, until the test lets it
/// land — the settle in flight a stop arrives during.
#[nest_rs_core::injectable]
#[derive(Default)]
struct HeldCommit;

/// A unit of it: an MCP operation's may be held, the request carrying it
/// never is.
struct HeldCommitUnit {
    operation: bool,
}

impl nest_rs_database::UnitOfWork for HeldCommit {
    fn open(
        &self,
        spec: &nest_rs_database::UnitSpec,
    ) -> Result<Box<dyn nest_rs_database::OpenUnit>, nest_rs_database::Unhonoured> {
        Ok(Box::new(HeldCommitUnit {
            operation: spec.unit.name() != nest_rs_http::unit::REQUEST.name(),
        }))
    }

    fn settle_net(&self) -> Duration {
        Duration::from_secs(16)
    }
}

impl nest_rs_database::OpenUnit for HeldCommitUnit {
    fn executor(&self) -> Arc<dyn nest_rs_database::Executor> {
        Arc::new(NoExecutor)
    }

    fn settle(
        self: Box<Self>,
        _ok: bool,
    ) -> std::pin::Pin<Box<dyn Future<Output = nest_rs_database::Settlement> + Send>> {
        let held = self.operation && HOLD_NEXT.swap(false, Ordering::SeqCst);
        let landed = tokio::spawn(async move {
            if held {
                SETTLE_STARTED.notify_one();
                SETTLE_RELEASED.notified().await;
                SETTLE_LANDED.store(true, Ordering::SeqCst);
            }
        });
        Box::pin(async move {
            drop(landed.await);
            nest_rs_database::Settlement::Settled
        })
    }
}

#[module(
    imports = [crate::open_mcp()],
    providers = [SettlingTools, HeldCommit as dyn nest_rs_database::UnitOfWork],
)]
struct SettlingModule;

/// A stop cuts the body, never the commit: one arriving while the unit
/// settles leaves the operation to end as it answered, once its commit landed.
/// rmcp sends a client nothing for a request it cancelled, as the protocol
/// asks, so the line says how it ended.
#[tokio::test]
async fn a_stop_arriving_while_its_unit_commits_lets_the_operation_end_as_it_answered() {
    let logs = LogCapture::install();
    let app = TestApp::for_module::<SettlingModule>()
        .await
        .expect("the module boots");
    let session = open_session(app.http(), SETTLING, None).await;
    HOLD_NEXT.store(true, Ordering::SeqCst);

    let call = call_method(
        app.http(),
        SETTLING,
        &session,
        None,
        "tools/call",
        json!({ "name": "quick", "arguments": {} }),
    );
    tokio::pin!(call);
    tokio::select! {
        body = &mut call => panic!("the call answered before its unit settled: {body}"),
        () = SETTLE_STARTED.notified() => {}
    }
    notify(
        app.http(),
        SETTLING,
        &session,
        None,
        "notifications/cancelled",
        json!({ "requestId": 99, "reason": "the user gave up" }),
    )
    .await;
    // Past the stop: an operation it cut would file its line now.
    tokio::time::sleep(Duration::from_millis(100)).await;
    SETTLE_RELEASED.notify_one();

    let line = operation_line(&logs, "quick").await;
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::OK),
        "the stop came once the operation had answered: {line:?}",
    );
    assert!(SETTLE_LANDED.load(Ordering::SeqCst), "its commit landed");
}

/// What [`OperationUnit`] ran around: the method of each request.
static AROUND_OPERATIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Brought by its module, around every MCP operation: a unit-stage
/// interceptor, outside the guards.
#[nest_rs_core::interceptor(stage = unit)]
struct OperationUnit;

impl nest_rs_core::Layer for OperationUnit {
    fn serves(&self, unit: nest_rs_core::operation_log::Unit) -> bool {
        unit.edge() == Some(nest_rs_core::Edge::Mcp)
    }
}

#[nest_rs_core::async_trait]
impl nest_rs_core::Interceptor for OperationUnit {
    async fn intercept(
        &self,
        cx: &mut nest_rs_core::UnitContext<'_>,
        next: nest_rs_core::Next<'_>,
    ) -> nest_rs_core::Outcome {
        let method = cx
            .switch_to::<nest_rs_mcp::McpView>()
            .expect("an MCP operation carries the MCP view")
            .method()
            .to_owned();
        AROUND_OPERATIONS.lock().expect("the log").push(method);
        next.run(cx).await
    }
}

const UNIT_STAGED: &str = "/mcp/unit-staged";

#[mcp(path = "/mcp/unit-staged")]
#[derive(Clone, Default)]
struct UnitStagedTools;

#[tools]
impl UnitStagedTools {
    /// Answer with a constant.
    #[tool]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

#[module(imports = [crate::open_mcp()], providers = [UnitStagedTools, OperationUnit])]
struct UnitStagedModule;

/// Every request is one operation unit, and the unit stage runs once around
/// it — a tool call included, never a second time around the tool's site.
#[tokio::test]
async fn a_unit_stage_interceptor_runs_once_around_every_request() {
    let app = TestApp::for_module::<UnitStagedModule>()
        .await
        .expect("a unit-stage interceptor beside an MCP host boots");
    let session = open_session(app.http(), UNIT_STAGED, None).await;
    let called = call_method(
        app.http(),
        UNIT_STAGED,
        &session,
        None,
        "tools/call",
        json!({ "name": "ping", "arguments": {} }),
    )
    .await;
    assert!(called.contains("pong"), "{called}");
    call_method(
        app.http(),
        UNIT_STAGED,
        &session,
        None,
        "tools/list",
        json!({}),
    )
    .await;

    assert_eq!(
        *AROUND_OPERATIONS.lock().expect("the log"),
        ["initialize", "tools/call", "tools/list"],
        "a notification is no unit, and a tool call runs the stage once",
    );
}
