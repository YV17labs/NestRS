//! `#[mcp]` self-mounts its endpoint at its declared path, deny-all when no
//! guard runs at its request.

use nest_rs_core::module;
use nest_rs_mcp::rmcp::transport::streamable_http_server::session::store::{
    SessionState, SessionStore, SessionStoreError,
};
use nest_rs_mcp::{ServerHandler, mcp, tool_handler, tool_router};
use nest_rs_testing::TestApp;
use poem::http::StatusCode;

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
/// as on.
#[tokio::test]
async fn a_browser_request_from_another_origin_is_refused_while_cors_is_off() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::for_module::<ListeningModule>()
        .await
        .expect("boots");
    let initialize = nest_rs_testing::mcp::initialize_request();

    let refused = nest_rs_testing::mcp::post_message_with(
        app.http(),
        LISTENING,
        None,
        None,
        &[("origin", "https://evil.example")],
        &initialize,
    )
    .await;
    refused.assert_status(StatusCode::FORBIDDEN);
    let event = logs.expect_one("nest_rs::mcp", "browser origin refused");
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("origin").as_deref(),
        Some("https://evil.example")
    );
    assert_eq!(event.field("expected").as_deref(), Some("http://localhost"));

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
    refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    refused.assert_header(poem::http::header::RETRY_AFTER, "5");
    refused.assert_content_type("application/problem+json");
    let problem: serde_json::Value = refused.0.into_body().into_json().await.expect("a problem");
    assert_eq!(problem["status"], 503, "{problem}");
    assert_eq!(problem["code"], "UNAVAILABLE", "{problem}");
    let event = logs.expect_one(
        "nest_rs::mcp",
        "mcp session refused: the mount holds its maximum",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("max_concurrent_sessions").as_deref(), Some("2"));

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
