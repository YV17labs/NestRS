//! `#[mcp]` self-mounts its endpoint at its declared path, deny-all when no
//! guard runs at its request.

use nest_rs_core::module;
use nest_rs_http::StatusCode;
use nest_rs_mcp::rmcp::transport::streamable_http_server::session::store::{
    SessionState, SessionStore, SessionStoreError,
};
use nest_rs_mcp::{ServerHandler, mcp, tool_handler, tool_router};
use nest_rs_testing::TestApp;

#[mcp]
#[derive(Clone)]
struct EchoTool;

#[tool_router(allow_empty)]
impl EchoTool {}

#[tool_handler]
impl ServerHandler for EchoTool {}

#[module(providers = [EchoTool])]
struct McpMountModule;

#[tokio::test]
async fn mcp_tool_self_mounts_and_fails_closed_without_a_guard() {
    let app = TestApp::for_module::<McpMountModule>()
        .await
        .expect("boots");

    // 401, not 404: mounted, and closed with no guard at its request.
    let resp = app.http().post("/mcp").send().await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    let resp = app.http().post("/elsewhere").send().await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

/// An empty `allowed_hosts` turns off rmcp's DNS-rebinding defence, invisibly to
/// a client: this warn, said when the boot mounts the path, is the whole control.
#[tokio::test]
async fn a_mount_with_no_allowlist_reports_that_host_validation_is_off() {
    let logs = nest_rs_testing::LogCapture::install();
    // The default allowlist is loopback; empty means `<PREFIX>_MCP__ALLOWED_HOSTS` was cleared.
    let _app = TestApp::builder()
        .provide(nest_rs_mcp::McpConfig {
            allowed_hosts: Vec::new(),
            ..nest_rs_mcp::McpConfig::default()
        })
        .module::<McpMountModule>()
        .build()
        .await
        .expect("boots");

    let event = logs.expect_one(
        "nest_rs::mcp",
        "mcp host allowlist is empty — inbound Host headers are not validated",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("reason").as_deref(),
        Some("host_validation_disabled"),
    );
}

const LISTENING: &str = "/mcp/listening";

#[nest_rs_mcp::mcp(path = "/mcp/listening")]
#[derive(Clone, Default)]
struct ListeningTools;

#[nest_rs_mcp::tools]
impl ListeningTools {
    #[tool(description = "Answers at once.")]
    #[public]
    async fn ping(&self) -> Result<String, nest_rs_mcp::McpError> {
        Ok("pong".to_owned())
    }
}

#[module(imports = [crate::open_mcp()], providers = [
    ListeningTools])]
struct ListeningModule;

/// The standalone `GET` stream has no end of its own: it ends cleanly at the
/// signal, so the client reconnects elsewhere, and its line files `cancelled`.
#[tokio::test]
async fn the_standalone_stream_ends_at_the_shutdown_signal() {
    let logs = nest_rs_testing::LogCapture::install();
    let window = std::time::Duration::from_secs(5);
    let serving = crate::serve_on_loopback::<ListeningModule>(window).await;
    let session = crate::open_raw_session(serving.port, LISTENING).await;
    let (mut stream, head) = crate::get_stream(serving.port, LISTENING, &session).await;
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "the stream opened: {head}"
    );

    let took = serving.stop().await;

    assert!(
        took < std::time::Duration::from_secs(1),
        "an idle client no longer holds the replica for its {window:?} window, took {took:?}",
    );
    let rest = crate::read_to_end(&mut stream).await;
    assert!(
        rest.ends_with("0\r\n\r\n"),
        "the stream ended with its last chunk rather than a cut: {rest:?}",
    );
    let lines: Vec<_> = logs
        .find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_http::unit::REQUEST.name(),
        )
        .into_iter()
        .filter(|line| line.field("method").as_deref() == Some("GET"))
        .collect();
    assert_eq!(lines.len(), 1, "one line for the stream: {lines:#?}");
    assert_eq!(lines[0].field("status").as_deref(), Some("200"));
    assert_eq!(
        lines[0].field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::CANCELLED),
    );
}

/// The MCP transport's Security Warning: a server validates `Origin` on every
/// incoming connection, refusing an invalid one `403` — with CORS off as much
/// as on. The line says the refusal by kind: the `Origin` sent and the `Host`
/// it was compared to are both the client's, and each carries a marker no
/// field may hold.
#[tokio::test]
async fn a_browser_request_from_another_origin_is_refused_while_cors_is_off() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<ListeningModule>()
        .await
        .expect("boots");
    let initialize = nest_rs_testing::mcp::initialize_request();

    app.http()
        .post(LISTENING)
        .header("host", "host-marker.example")
        .header("origin", "https://origin-marker.example")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body_json(&initialize)
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    let event = logs.expect_one("nest_rs::mcp", "browser origin refused");
    assert_eq!(event.level, "warn");
    for (field, value) in &event.fields {
        assert!(!value.contains("marker"), "{field} = {value}");
    }
    assert_eq!(event.field("reason").as_deref(), Some("scheme_differs"));
    assert_eq!(event.field("scheme").as_deref(), Some("https"));
    assert_eq!(event.field("trusted_proxy").as_deref(), Some("false"));
    let hint = event.field("hint").unwrap_or_default();
    for setting in ["CORS_ORIGINS", "TRUSTED_PROXIES"] {
        assert!(
            hint.contains(&nest_rs_config::var_name("http", setting)),
            "{hint}"
        );
    }

    for origin in [&[][..], &[("origin", "http://localhost")][..]] {
        nest_rs_testing::mcp::post_message_with(
            app.http(),
            LISTENING,
            None,
            None,
            origin,
            &initialize,
        )
        .await
        .assert_status_is_ok();
    }
}

/// [`ListeningModule`]'s mount, served on `config`.
async fn listening_on(config: nest_rs_mcp::McpConfig) -> TestApp {
    TestApp::builder()
        .provide(config)
        .module::<ListeningModule>()
        .build()
        .await
        .expect("boots")
}

fn two_sessions() -> nest_rs_mcp::McpConfig {
    nest_rs_mcp::McpConfig {
        max_concurrent_sessions: 2,
        ..nest_rs_mcp::McpConfig::default()
    }
}

/// rmcp refuses a `Host` outside the allowlist with `403`, and its own line,
/// which quotes the `Host`, is under its clamp: the mount files the denial,
/// naming the variable that admits a host and never the one refused.
#[tokio::test]
async fn a_host_outside_the_allowlist_is_refused_and_said_without_its_value() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = listening_on(nest_rs_mcp::McpConfig::default()).await;

    app.http()
        .post(LISTENING)
        .header("host", "rebound.example")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body_json(&serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);

    let event = logs.expect_one(
        "nest_rs::mcp",
        "mcp request refused: its Host is outside the allowlist",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("reason").as_deref(), Some("host_not_allowed"));
    let hint = event.field("hint").expect("a hint");
    assert!(
        hint.contains(&nest_rs_config::var_name("mcp", "ALLOWED_HOSTS")),
        "{hint}"
    );
    assert!(
        event
            .fields
            .values()
            .all(|value| !value.contains("rebound")),
        "the refused host is quoted: {:?}",
        event.fields
    );
}

/// A JSON-RPC request, id `7`, calling `method` with `params`.
fn request(method: &str, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params })
}

/// rmcp's transport refuses a request before any handler runs in sentences
/// quoting what it carried — the `MCP-Protocol-Version` it named, an
/// `Mcp-Method` or `Mcp-Name` header its body contradicts: each is answered
/// by kind, its status, and a JSON-RPC error's code and id, kept.
#[tokio::test]
async fn a_request_the_transport_refuses_is_answered_without_what_it_carried() {
    let app = listening_on(nest_rs_mcp::McpConfig::default()).await;
    let session = nest_rs_testing::mcp::open_session(app.http(), LISTENING, None).await;
    let meta = serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": { "name": "nest-rs-mcp", "version": "0" },
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    let cases = [
        (
            vec![("mcp-protocol-version", "marked-version")],
            request("tools/list", serde_json::json!({})),
            None,
        ),
        (
            vec![
                ("mcp-protocol-version", "2026-07-28"),
                ("mcp-method", "marked/method"),
            ],
            request("tools/list", serde_json::json!({ "_meta": meta })),
            Some(-32020),
        ),
        (
            vec![
                ("mcp-protocol-version", "2026-07-28"),
                ("mcp-method", "tools/call"),
                ("mcp-name", "marked"),
            ],
            request(
                "tools/call",
                serde_json::json!({ "name": "ping", "arguments": {}, "_meta": meta }),
            ),
            Some(-32020),
        ),
    ];
    for (headers, body, code) in cases {
        let response = nest_rs_testing::mcp::post_message_with(
            app.http(),
            LISTENING,
            Some(&session),
            None,
            &headers,
            &body,
        )
        .await;
        response.assert_status(StatusCode::BAD_REQUEST);
        let said = response.0.into_body().into_string().await.expect("a body");
        assert!(!said.contains("marked"), "{said}");
        if let Some(code) = code {
            let said: serde_json::Value = serde_json::from_str(&said).expect("a JSON-RPC error");
            assert_eq!(said["error"]["code"], serde_json::json!(code), "{said}");
            assert_eq!(said["id"], serde_json::json!(7), "{said}");
        }
    }
}

/// rmcp's transport refuses some requests in constant sentences, quoting
/// nothing the request carried — a missing `Accept`, a missing or unknown
/// session, a body past the cap, the `_meta` keys the specification fixes:
/// each is kept for the client, as the problem's `detail` or the JSON-RPC
/// error's `message`.
#[tokio::test]
async fn a_transport_refusal_quoting_nothing_keeps_its_sentence() {
    let app = listening_on(nest_rs_mcp::McpConfig {
        max_request_body_bytes: 256,
        ..nest_rs_mcp::McpConfig::default()
    })
    .await;
    let detail = |response: nest_rs_testing::TestResponse| async move {
        let body = response.0.into_body().into_string().await.expect("a body");
        let problem: serde_json::Value = serde_json::from_str(&body).expect("a problem document");
        problem["detail"].clone()
    };

    let accepting_json_only = app
        .http()
        .post(LISTENING)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .body_json(&request("tools/list", serde_json::json!({})))
        .send()
        .await;
    accepting_json_only.assert_status(StatusCode::NOT_ACCEPTABLE);
    assert_eq!(
        detail(accepting_json_only).await,
        "Not Acceptable: Client must accept both application/json and text/event-stream",
    );

    let sessionless = app
        .http()
        .get(LISTENING)
        .header("host", "localhost")
        .header("accept", "text/event-stream")
        .send()
        .await;
    sessionless.assert_status(StatusCode::BAD_REQUEST);
    assert_eq!(
        detail(sessionless).await,
        "Bad Request: Session ID is required"
    );

    let unknown_session = nest_rs_testing::mcp::post_message(
        app.http(),
        LISTENING,
        Some("no-such-session"),
        None,
        &request("tools/list", serde_json::json!({})),
    )
    .await;
    unknown_session.assert_status(StatusCode::NOT_FOUND);
    assert_eq!(
        detail(unknown_session).await,
        "Not Found: Session not found"
    );

    let oversized = nest_rs_testing::mcp::post_message(
        app.http(),
        LISTENING,
        None,
        None,
        &request(
            "tools/list",
            serde_json::json!({ "padding": "x".repeat(512) }),
        ),
    )
    .await;
    oversized.assert_status(StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        detail(oversized).await,
        "Payload Too Large: request body exceeds 256 bytes"
    );

    let without_meta = nest_rs_testing::mcp::post_message_with(
        app.http(),
        LISTENING,
        None,
        None,
        &[
            ("mcp-protocol-version", "2026-07-28"),
            ("mcp-method", "tools/list"),
        ],
        &request("tools/list", serde_json::json!({})),
    )
    .await;
    without_meta.assert_status(StatusCode::BAD_REQUEST);
    let body = without_meta
        .0
        .into_body()
        .into_string()
        .await
        .expect("a body");
    let refused: serde_json::Value = serde_json::from_str(&body).expect("a JSON-RPC error");
    assert_eq!(
        refused["error"]["code"],
        serde_json::json!(-32602),
        "{refused}"
    );
    assert_eq!(
        refused["error"]["message"],
        "Invalid params: request _meta is missing or has malformed required fields: \
         io.modelcontextprotocol/protocolVersion, io.modelcontextprotocol/clientCapabilities",
        "{refused}",
    );
}

/// How many [`CountedTools`] hosts the mount built: one per session, and one
/// per lookup rmcp's transport makes. Each test runs in its own process.
static BUILT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Counts its own construction.
#[derive(Clone)]
struct Built;

impl Default for Built {
    fn default() -> Self {
        BUILT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self
    }
}

const COUNTED: &str = "/mcp/counted";

#[nest_rs_mcp::mcp(path = "/mcp/counted")]
#[derive(Clone, Default)]
struct CountedTools {
    built: Built,
}

#[nest_rs_mcp::tools]
impl CountedTools {
    #[tool(description = "Answers at once.")]
    #[public]
    async fn ping(&self) -> Result<String, nest_rs_mcp::McpError> {
        let Built = &self.built;
        Ok("pong".to_owned())
    }
}

#[module(imports = [crate::open_mcp()], providers = [CountedTools])]
struct CountedModule;

/// A `tools/call` at the standard-headers version, as a client sends it.
async fn standard_call(app: &TestApp, name: &str) -> nest_rs_testing::TestResponse {
    let meta = serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": { "name": "nest-rs-mcp", "version": "0" },
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    nest_rs_testing::mcp::post_message_with(
        app.http(),
        COUNTED,
        None,
        None,
        &[
            ("mcp-protocol-version", "2026-07-28"),
            ("mcp-method", "tools/call"),
            ("mcp-name", name),
        ],
        &request(
            "tools/call",
            serde_json::json!({ "name": name, "arguments": {}, "_meta": meta }),
        ),
    )
    .await
}

/// rmcp 3.5's transport looks up the schema of the tool a `tools/call` at
/// the standard-headers version names, building a whole handler to ask, and
/// keeps the answer — a miss included — in a map nothing bounds: one name per
/// request grows it for good. A name no host on the mount declares is
/// answered before rmcp sees it, as rmcp's own handler would answer it.
#[tokio::test]
async fn a_tool_call_naming_no_declared_tool_never_reaches_rmcp_s_schema_cache() {
    let app = TestApp::for_module::<CountedModule>()
        .await
        .expect("the counted mount boots");
    let before = BUILT.load(std::sync::atomic::Ordering::SeqCst);

    let mut answers = Vec::new();
    for n in 0..16 {
        let response = standard_call(&app, &format!("unknown_{n}")).await;
        let status = response.0.status();
        let body = response.0.into_body().into_string().await.expect("a body");
        answers.push((status, body));
    }
    assert_eq!(
        BUILT.load(std::sync::atomic::Ordering::SeqCst) - before,
        0,
        "an undeclared name built a handler for rmcp's schema lookup, which keeps it",
    );
    for (status, body) in answers {
        assert_eq!(status, StatusCode::OK, "{body}");
        let refused = nest_rs_testing::mcp::result(&body);
        assert_eq!(refused["id"], serde_json::json!(7), "{refused}");
        assert_eq!(
            refused["error"]["code"],
            serde_json::json!(-32602),
            "{refused}"
        );
        assert_eq!(refused["error"]["message"], "tool not found", "{refused}");
    }

    let declared = standard_call(&app, "ping").await;
    let body = declared.0.into_body().into_string().await.expect("a body");
    assert!(
        body.contains("pong"),
        "a declared tool still reaches rmcp: {body}"
    );
}

/// A `tools/call` at the standard-headers version naming `name`, whose id is
/// `id` as written — raw JSON, a repeated key included — or absent.
async fn raw_standard_call(
    app: &TestApp,
    name: &str,
    id: Option<&str>,
) -> nest_rs_testing::TestResponse {
    let id = id.map_or_else(String::new, |id| format!("{id},"));
    let body = format!(
        r#"{{"jsonrpc":"2.0",{id}"method":"tools/call","params":{{"name":"{name}","arguments":{{}},"_meta":{{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{{"name":"nest-rs-mcp","version":"0"}},"io.modelcontextprotocol/clientCapabilities":{{}}}}}}}}"#
    );
    app.http()
        .post(COUNTED)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "tools/call")
        .header("mcp-name", name)
        .body(body)
        .send()
        .await
}

/// rmcp takes a call whose id it cannot read — none, an object, `null`, a
/// fraction, a repeated key — for a notification, and still looks its tool
/// up: the gate decides on the method and the name alone. Such a call is
/// input the server cannot accept, refused `400` with an error carrying no
/// id, and files no operation, as rmcp's own transport refusals file none.
#[tokio::test]
async fn a_tool_call_with_no_id_rmcp_reads_never_reaches_rmcp_s_schema_cache() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<CountedModule>()
        .await
        .expect("the counted mount boots");
    let before = BUILT.load(std::sync::atomic::Ordering::SeqCst);

    let mut answers = Vec::new();
    for (case, id) in [
        None,
        Some(r#""id":{}"#),
        Some(r#""id":null"#),
        Some(r#""id":1.5"#),
        Some(r#""id":7,"id":8"#),
    ]
    .into_iter()
    .enumerate()
    {
        for n in 0..4 {
            let response = raw_standard_call(&app, &format!("unknown_{case}_{n}"), id).await;
            let status = response.0.status();
            let body = response.0.into_body().into_string().await.expect("a body");
            answers.push((id, status, body));
        }
    }
    assert_eq!(
        BUILT.load(std::sync::atomic::Ordering::SeqCst) - before,
        0,
        "an undeclared name built a handler for rmcp's schema lookup, which keeps it",
    );
    for (id, status, body) in answers {
        assert_eq!(status, StatusCode::BAD_REQUEST, "{id:?}: {body}");
        let refused: serde_json::Value = serde_json::from_str(&body).expect("a JSON-RPC error");
        assert_eq!(refused.get("id"), None, "{id:?}: {refused}");
        assert_eq!(
            refused["error"]["code"],
            serde_json::json!(-32602),
            "{refused}"
        );
        assert_eq!(refused["error"]["message"], "tool not found", "{refused}");
    }
    assert!(
        logs.find(
            nest_rs_core::operation_log::TARGET,
            nest_rs_mcp::unit::OPERATION.name(),
        )
        .is_empty(),
        "an input refused for its shape is no operation",
    );
}

/// A call the gate answers is still the operation the handler would have
/// answered: it opens the `mcp.operation` span and files the line — the
/// protocol's method, `_OTHER` for the tool, `error` — a unit of the request.
#[tokio::test]
async fn a_tool_call_answered_before_rmcp_files_its_operation_line() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<CountedModule>()
        .await
        .expect("the counted mount boots");

    standard_call(&app, "marked_tool")
        .await
        .assert_status_is_ok();

    let lines = logs.find(
        nest_rs_core::operation_log::TARGET,
        nest_rs_mcp::unit::OPERATION.name(),
    );
    let [line] = lines.as_slice() else {
        panic!("one line for the call: {lines:#?}");
    };
    assert_eq!(line.field("method").as_deref(), Some("tools/call"));
    assert_eq!(
        line.field("operation").as_deref(),
        Some(nest_rs_core::operation_log::OTHER),
    );
    assert_eq!(
        line.field("outcome").as_deref(),
        Some(nest_rs_core::operation_log::ERROR),
    );
    let request = logs.expect_one(
        nest_rs_core::operation_log::TARGET,
        nest_rs_http::unit::REQUEST.name(),
    );
    assert_eq!(line.trace_id, request.trace_id, "the request's trace");
    assert_ne!(line.span_id, request.span_id, "a unit of its own");

    let span = logs
        .spans()
        .into_iter()
        .find(|span| span.name == nest_rs_mcp::unit::OPERATION.name())
        .expect("an mcp.operation span");
    assert_eq!(span.field("mcp.method.name").as_deref(), Some("tools/call"));
    assert_eq!(
        span.field("mcp.operation.name").as_deref(),
        Some("marked_tool"),
        "the conventions' tool name, as the handler's span records it",
    );
    assert_eq!(
        span.field("error.type").as_deref(),
        Some(nest_rs_core::operation_log::ERROR),
    );
    assert_eq!(span.field("parent_span_id"), request.span_id);
}

/// What [`GateWatch`] ran around, as `<method> <name>`. This test's alone:
/// nextest runs each test in its own process.
static WATCHED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Around every MCP request: a unit-stage interceptor its module brings.
#[nest_rs_core::interceptor(stage = unit)]
struct GateWatch;

impl nest_rs_core::Layer for GateWatch {
    fn serves(&self, unit: nest_rs_core::operation_log::Unit) -> bool {
        unit.edge() == Some(nest_rs_core::Edge::Mcp)
    }
}

#[nest_rs_core::async_trait]
impl nest_rs_core::Interceptor for GateWatch {
    async fn intercept(
        &self,
        cx: &mut nest_rs_core::UnitContext<'_>,
        next: nest_rs_core::Next<'_>,
    ) -> nest_rs_core::Outcome {
        let view = cx
            .switch_to::<nest_rs_mcp::McpView>()
            .expect("an MCP operation carries the MCP view");
        let seen = format!("{} {}", view.method(), view.name().unwrap_or("-"));
        WATCHED.lock().expect("the log").push(seen);
        next.run(cx).await
    }
}

#[module(imports = [crate::open_mcp()], providers = [CountedTools, GateWatch])]
struct WatchedModule;

/// A call the gate answers runs the unit stage the handler runs around every
/// request, as the same call in an older session does — and an input refused
/// for its shape, which is no operation, runs none.
#[tokio::test]
async fn a_tool_call_answered_before_rmcp_runs_the_unit_stage() {
    let app = TestApp::for_module::<WatchedModule>()
        .await
        .expect("a unit-stage interceptor beside the counted mount boots");

    let answered = standard_call(&app, "marked_tool").await;
    let body = answered.0.into_body().into_string().await.expect("a body");
    let refused = nest_rs_testing::mcp::result(&body);
    assert_eq!(refused["id"], serde_json::json!(7), "{refused}");
    assert_eq!(refused["error"]["message"], "tool not found", "{refused}");
    assert_eq!(
        std::mem::take(&mut *WATCHED.lock().expect("the log")),
        ["tools/call marked_tool"],
    );

    let declared = standard_call(&app, "ping").await;
    let body = declared.0.into_body().into_string().await.expect("a body");
    assert!(body.contains("pong"), "{body}");
    assert_eq!(
        std::mem::take(&mut *WATCHED.lock().expect("the log")),
        ["tools/call ping"],
    );

    let shapeless = raw_standard_call(&app, "unread_tool", None).await;
    assert_eq!(shapeless.0.status(), StatusCode::BAD_REQUEST);
    assert!(
        WATCHED.lock().expect("the log").is_empty(),
        "an input refused for its shape is no operation",
    );
}

/// rmcp refuses a `Host` outside its allowlist before it looks any schema
/// up: such a call is left to that refusal, so a page a rebound name serves
/// learns nothing of which tools exist.
#[tokio::test]
async fn a_tool_call_from_a_host_rmcp_refuses_is_refused_for_its_host_first() {
    let app = TestApp::for_module::<CountedModule>()
        .await
        .expect("the counted mount boots");
    let rebound = app
        .http()
        .post(COUNTED)
        .header("host", "rebound.example")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "tools/call")
        .header("mcp-name", "unknown")
        .body_json(&request(
            "tools/call",
            serde_json::json!({ "name": "unknown", "arguments": {} }),
        ))
        .send()
        .await;
    rebound.assert_status(StatusCode::FORBIDDEN);
}

/// The app's session store, failing to load every session.
struct StoreDown;

#[async_trait::async_trait]
impl SessionStore for StoreDown {
    async fn load(&self, _: &str) -> Result<Option<SessionState>, SessionStoreError> {
        Err("the store is down".into())
    }

    async fn store(&self, _: &str, _: &SessionState) -> Result<(), SessionStoreError> {
        Ok(())
    }

    async fn delete(&self, _: &str) -> Result<(), SessionStoreError> {
        Ok(())
    }
}

/// A session the app's store fails to restore is answered `500`, and said by
/// the mount without the session it names: rmcp's own line names it, and is
/// under its clamp.
#[tokio::test]
async fn a_session_the_store_fails_to_restore_is_said_without_the_session() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::builder()
        .provide_dyn::<dyn SessionStore>(std::sync::Arc::new(StoreDown))
        .module::<ListeningModule>()
        .build()
        .await
        .expect("boots");

    nest_rs_testing::mcp::post_message_with(
        app.http(),
        LISTENING,
        Some("named-by-a-client"),
        None,
        &[],
        &serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    )
    .await
    .assert_status(StatusCode::INTERNAL_SERVER_ERROR);

    let event = logs.expect_one("nest_rs::mcp", "mcp session store failed");
    assert_eq!(event.level, "error");
    assert_eq!(event.field("call").as_deref(), Some("load"));
    assert_eq!(event.field("error").as_deref(), Some("the store is down"));
    // The capture records rmcp's own lines too, which the clamp keeps off a
    // subscriber the framework ships.
    let quoting: Vec<_> = logs
        .events()
        .into_iter()
        .filter(|event| event.target.starts_with("nest_rs"))
        .filter(|event| {
            event
                .fields
                .values()
                .any(|value| value.contains("named-by-a-client"))
        })
        .collect();
    assert!(
        quoting.is_empty(),
        "the session a client named is quoted: {quoting:#?}"
    );
}

/// A session outlives the connection that opened it, so the connection cap
/// never counts it: the mount's own cap does.
#[tokio::test]
async fn an_initialize_past_the_session_cap_is_refused_until_a_session_closes() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = listening_on(two_sessions()).await;
    let first = nest_rs_testing::mcp::open_session(app.http(), LISTENING, None).await;
    nest_rs_testing::mcp::open_session(app.http(), LISTENING, None).await;

    let refused = nest_rs_testing::mcp::post_message(
        app.http(),
        LISTENING,
        None,
        None,
        &nest_rs_testing::mcp::initialize_request(),
    )
    .await;
    assert_refused_at_the_cap(refused).await;
    expect_the_cap_said(&logs, "2");

    let answered = nest_rs_testing::mcp::call_method(
        app.http(),
        LISTENING,
        &first,
        None,
        "tools/list",
        serde_json::json!({}),
    )
    .await;
    assert!(
        nest_rs_testing::mcp::result(&answered)
            .get("result")
            .is_some(),
        "a session already open is served at the cap: {answered}"
    );

    app.http()
        .delete(LISTENING)
        .header("host", "localhost")
        .header("mcp-session-id", first.as_str())
        .send()
        .await
        .assert_status(StatusCode::ACCEPTED);
    nest_rs_testing::mcp::open_session(app.http(), LISTENING, None).await;
}

/// The cap's refusal: `503`, a wait to take, and the `UNAVAILABLE` problem.
async fn assert_refused_at_the_cap(refused: nest_rs_testing::TestResponse) {
    refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    refused.assert_header(nest_rs_http::header::RETRY_AFTER, "5");
    refused.assert_content_type("application/problem+json");
    let problem: serde_json::Value = refused.json().await.value().deserialize();
    assert_eq!(problem["status"], 503, "{problem}");
    assert_eq!(problem["code"], "UNAVAILABLE", "{problem}");
}

/// The cap's one line, naming the maximum `max` it holds.
fn expect_the_cap_said(logs: &nest_rs_testing::LogCapture, max: &str) {
    let event = logs.expect_one(
        "nest_rs::mcp",
        "mcp session refused: the mount holds its maximum",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("max_concurrent_sessions").as_deref(), Some(max));
}

fn one_session() -> nest_rs_mcp::McpConfig {
    nest_rs_mcp::McpConfig {
        max_concurrent_sessions: 1,
        ..nest_rs_mcp::McpConfig::default()
    }
}

/// The app's session store, holding every session a client names, as one
/// another instance stored.
struct StoredElsewhere;

#[async_trait::async_trait]
impl SessionStore for StoredElsewhere {
    async fn load(&self, _: &str) -> Result<Option<SessionState>, SessionStoreError> {
        let state = serde_json::json!({
            "initialize_params": {
                "protocolVersion": nest_rs_testing::mcp::PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "elsewhere", "version": "0" },
            },
        });
        Ok(Some(serde_json::from_value(state)?))
    }

    async fn store(&self, _: &str, _: &SessionState) -> Result<(), SessionStoreError> {
        Ok(())
    }

    async fn delete(&self, _: &str) -> Result<(), SessionStoreError> {
        Ok(())
    }
}

/// A session restored from the app's store takes a place like one opened
/// here: past the cap, the session manager refuses it, and the client reads
/// the cap's refusal rather than rmcp's `500`.
#[tokio::test]
async fn a_session_restored_past_the_cap_is_refused_as_the_cap_refuses() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::builder()
        .provide(one_session())
        .provide_dyn::<dyn SessionStore>(std::sync::Arc::new(StoredElsewhere))
        .module::<ListeningModule>()
        .build()
        .await
        .expect("boots");
    nest_rs_testing::mcp::open_session(app.http(), LISTENING, None).await;

    let refused = nest_rs_testing::mcp::post_message(
        app.http(),
        LISTENING,
        Some("stored-elsewhere"),
        None,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await;

    assert_refused_at_the_cap(refused).await;
    expect_the_cap_said(&logs, "1");
}

/// An `initialize` whose body rmcp can read only once `released` fires; it
/// says on `reading` when rmcp starts to, past every check in front of it.
fn held_initialize(
    reading: tokio::sync::oneshot::Sender<()>,
    released: tokio::sync::oneshot::Receiver<()>,
) -> nest_rs_http::Body {
    let initialize = serde_json::to_vec(&nest_rs_testing::mcp::initialize_request())
        .expect("an initialize serializes");
    nest_rs_http::Body::from_stream(futures_util::stream::once(async move {
        let _ = reading.send(());
        let _ = released.await;
        Ok::<_, std::io::Error>(nest_rs_http::Bytes::from(initialize))
    }))
}

/// Two `initialize`s racing for the last place both pass the check in front
/// of rmcp, which reads no body while a place is free; the session manager
/// refuses the second, which the client reads as the check's refusal.
#[tokio::test]
async fn the_loser_of_a_race_for_the_last_place_is_refused_as_the_cap_refuses() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = listening_on(one_session()).await;
    let initialize = |body: nest_rs_http::Body| {
        app.http()
            .post(LISTENING)
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(body)
            .send()
    };
    let (first_reading, first_read) = tokio::sync::oneshot::channel();
    let (first_release, first_released) = tokio::sync::oneshot::channel();
    let (second_reading, second_read) = tokio::sync::oneshot::channel();
    let (second_release, second_released) = tokio::sync::oneshot::channel();

    let (first, second, ()) = tokio::join!(
        initialize(held_initialize(first_reading, first_released)),
        initialize(held_initialize(second_reading, second_released)),
        async {
            first_read.await.expect("rmcp reads the first body");
            second_read.await.expect("rmcp reads the second body");
            let _ = first_release.send(());
            let _ = second_release.send(());
        },
    );

    let (served, refused) = if first.0.status() == StatusCode::OK {
        (first, second)
    } else {
        (second, first)
    };
    served.assert_status_is_ok();
    assert_refused_at_the_cap(refused).await;
    expect_the_cap_said(&logs, "1");
}

/// The cap counts sessions, so what opens none is served at it: a
/// `2026-07-28` request carries its lifecycle inline.
#[tokio::test]
async fn a_stateless_request_is_served_while_the_sessions_are_full() {
    let app = listening_on(two_sessions()).await;
    for _ in 0..2 {
        nest_rs_testing::mcp::open_session(app.http(), LISTENING, None).await;
    }
    let version = nest_rs_mcp::ProtocolVersion::LATEST;

    let discovered = app
        .http()
        .post(LISTENING)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", version.as_str())
        .header("mcp-method", "server/discover")
        .body_json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "server/discover",
            "params": { "_meta": {
                "io.modelcontextprotocol/protocolVersion": version.as_str(),
                "io.modelcontextprotocol/clientInfo": { "name": "nest-rs-mcp", "version": "0" },
                "io.modelcontextprotocol/clientCapabilities": {},
            } },
        }))
        .send()
        .await;
    discovered.assert_status_is_ok();
}

#[tokio::test(start_paused = true)]
async fn a_session_left_idle_past_its_timeout_is_closed_and_frees_its_place() {
    let app = listening_on(nest_rs_mcp::McpConfig {
        max_concurrent_sessions: 1,
        session_idle_timeout: std::time::Duration::from_secs(1),
        // sse-stream re-arms its keep-alive on the std clock, which a paused
        // tokio clock would flood past.
        sse_keep_alive: None,
        ..nest_rs_mcp::McpConfig::default()
    })
    .await;
    let idle = nest_rs_testing::mcp::open_session(app.http(), LISTENING, None).await;

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    nest_rs_testing::mcp::post_message(
        app.http(),
        LISTENING,
        Some(&idle),
        None,
        &serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await
    .assert_status(StatusCode::NOT_FOUND);
    nest_rs_testing::mcp::open_session(app.http(), LISTENING, None).await;
}

/// The event stream answering one request on a session of a mount served
/// with `sse_retry`.
async fn a_session_request_stream(sse_retry: Option<std::time::Duration>) -> String {
    let app = listening_on(nest_rs_mcp::McpConfig {
        sse_retry,
        ..nest_rs_mcp::McpConfig::default()
    })
    .await;
    let session = nest_rs_testing::mcp::open_session(app.http(), LISTENING, None).await;
    nest_rs_testing::mcp::call_method(
        app.http(),
        LISTENING,
        &session,
        None,
        "tools/list",
        serde_json::json!({}),
    )
    .await
}

#[tokio::test]
async fn the_configured_sse_retry_reaches_a_session_stream() {
    let silent = a_session_request_stream(None).await;
    assert!(
        !silent.contains("retry:"),
        "no retry is advertised: {silent:?}"
    );

    let seven = a_session_request_stream(Some(std::time::Duration::from_secs(7))).await;
    assert!(
        seven.contains("retry: 7000"),
        "the configured retry: {seven:?}"
    );
}
