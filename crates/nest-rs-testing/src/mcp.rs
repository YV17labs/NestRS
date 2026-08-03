//! Driving an MCP endpoint over streamable HTTP.
//!
//! Every MCP operation is a JSON-RPC message that has to carry the same three
//! headers and follow the same `initialize` → `notifications/initialized` →
//! *operation* order. Hand-rolling that per suite means each copy re-encodes
//! the protocol version and the header triple, and they drift — so it lives
//! here once, next to the [`TestClient`] it drives.
//!
//! Nothing here depends on `nest-rs-mcp`: it is JSON over HTTP, so it works
//! against a [`TestApp`](crate::TestApp)'s client and against a bare
//! `endpoint(..)` mount alike.

use poem::Endpoint;
use poem::test::{TestClient, TestResponse};
use serde_json::{Value, json};

/// The protocol version every suite negotiates. One constant so a bump is one
/// edit, not a grep.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// The `initialize` request body, declaring no client capabilities.
pub fn initialize_request() -> Value {
    initialize_request_with(json!({}))
}

/// [`initialize_request`] with explicit client `capabilities` — what a suite
/// needs to reach a capability-gated method (`tasks/*` answers `-32021`
/// *Missing Required Client Capability* until the client declares
/// `extensions: { "io.modelcontextprotocol/tasks": {} }`).
pub fn initialize_request_with(capabilities: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": capabilities,
            "clientInfo": { "name": "nest-rs-testing", "version": "0" }
        }
    })
}

/// POST one JSON-RPC message with the headers streamable HTTP requires.
/// `session` is the id [`open_session`] returned; `bearer` is the raw
/// `authorization` value (`"Bearer …"`), omitted for an anonymous call.
pub async fn post_message<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    session: Option<&str>,
    bearer: Option<&str>,
    body: &Value,
) -> TestResponse {
    let mut request = client
        .post(path)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body_json(body);
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    if let Some(bearer) = bearer {
        request = request.header("authorization", bearer);
    }
    request.send().await
}

/// Run `initialize` + `notifications/initialized` and return the session id.
/// Panics if the endpoint refuses the handshake — a suite that expects a
/// refusal should assert on [`post_message`] directly.
pub async fn open_session<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    bearer: Option<&str>,
) -> String {
    open_session_with(client, path, bearer, json!({})).await
}

/// [`open_session`] declaring client `capabilities` on the handshake.
pub async fn open_session_with<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    bearer: Option<&str>,
    capabilities: Value,
) -> String {
    let init = post_message(
        client,
        path,
        None,
        bearer,
        &initialize_request_with(capabilities),
    )
    .await;
    init.assert_status_is_ok();
    let session = init
        .0
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("initialize returns a session id")
        .to_owned();

    post_message(
        client,
        path,
        Some(&session),
        bearer,
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;

    session
}

/// Send one JSON-RPC **request** on an open session and return the raw
/// response body. `params` is the method's params object (`json!({})` when it
/// takes none).
///
/// The tool-shaped [`call_tool`] is the common case; this is the general one,
/// for the rest of the MCP surface — `prompts/get`, `resources/read`,
/// `completion/complete`, `tasks/get`, a custom method.
pub async fn call_method<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    session: &str,
    bearer: Option<&str>,
    method: &str,
    params: Value,
) -> String {
    let response = post_message(
        client,
        path,
        Some(session),
        bearer,
        &json!({ "jsonrpc": "2.0", "id": 99, "method": method, "params": params }),
    )
    .await;
    response
        .0
        .into_body()
        .into_string()
        .await
        .expect("a JSON-RPC response body")
}

/// Send one JSON-RPC **notification** (no `id`, no response) on an open
/// session.
pub async fn notify<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    session: &str,
    bearer: Option<&str>,
    method: &str,
    params: Value,
) {
    post_message(
        client,
        path,
        Some(session),
        bearer,
        &json!({ "jsonrpc": "2.0", "method": method, "params": params }),
    )
    .await;
}

/// Drive the full handshake and call `tool` with no arguments, returning the
/// response body — what a suite asserts on.
pub async fn call_tool<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    tool: &str,
    bearer: Option<&str>,
) -> String {
    let session = open_session(client, path, bearer).await;
    call_method(
        client,
        path,
        &session,
        bearer,
        "tools/call",
        json!({ "name": tool, "arguments": {} }),
    )
    .await
}
