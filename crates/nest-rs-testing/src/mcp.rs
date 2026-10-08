//! Driving an MCP endpoint over streamable HTTP, through a
//! [`TestApp`](crate::TestApp)'s [`TestClient`] or a bare `endpoint(..)` mount.

use poem::Endpoint;
use poem::test::{TestClient, TestResponse};
use serde_json::{Value, json};

/// The protocol version every suite negotiates — rmcp's own `LATEST`, pinned by
/// `mcp::the_driver_negotiates_the_sdk_latest`.
///
/// Not `ProtocolVersion::STANDARD_HEADERS` (`2026-07-28`): SEP-2567 serves that
/// revision statelessly, retiring the `mcp-session-id` model [`open_session`]
/// implements.
pub const PROTOCOL_VERSION: &str = "2025-11-25";

/// The `initialize` request body, declaring no client capabilities.
pub fn initialize_request() -> Value {
    initialize_request_with(json!({}))
}

/// [`initialize_request`] with explicit client `capabilities`, to reach a
/// capability-gated method (`tasks/*` answers `-32021` until the client
/// declares `extensions: { "io.modelcontextprotocol/tasks": {} }`).
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
    post_message_with(client, path, session, bearer, &[], body).await
}

/// [`post_message`] carrying extra request headers.
///
/// Prefer it to a hand-rolled POST, which drops the `host` header rmcp's
/// DNS-rebinding defence requires and answers `400`.
pub async fn post_message_with<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    session: Option<&str>,
    bearer: Option<&str>,
    headers: &[(&str, &str)],
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
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    request.send().await
}

/// Run `initialize` + `notifications/initialized` and return the session id.
/// Panics if the endpoint refuses the handshake; assert a refusal on
/// [`post_message`].
pub async fn open_session<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    bearer: Option<&str>,
) -> String {
    open_session_with(client, path, bearer, &[], json!({})).await
}

/// [`open_session`] declaring client `capabilities` on the handshake, under
/// caller-supplied `headers`.
pub async fn open_session_with<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    bearer: Option<&str>,
    headers: &[(&str, &str)],
    capabilities: Value,
) -> String {
    let init = post_message_with(
        client,
        path,
        None,
        bearer,
        headers,
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

    post_message_with(
        client,
        path,
        Some(&session),
        bearer,
        headers,
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;

    session
}

/// Run `initialize` and return the raw response body, to assert what the
/// handshake advertises.
pub async fn initialize<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    bearer: Option<&str>,
) -> String {
    post_message(client, path, None, bearer, &initialize_request())
        .await
        .0
        .into_body()
        .into_string()
        .await
        .expect("an initialize response body")
}

/// Decode the JSON-RPC response carried by a body from any of the calls here.
///
/// The SSE stream opens with an empty `data:` keep-alive frame, so the payload
/// is the first frame carrying a `result` or an `error`.
///
/// # Panics
///
/// If no frame carries either.
pub fn result(body: &str) -> Value {
    std::iter::once(body)
        .chain(body.lines().filter_map(|line| line.strip_prefix("data: ")))
        .filter_map(|frame| serde_json::from_str::<Value>(frame).ok())
        .find(|value| value.get("result").is_some() || value.get("error").is_some())
        .unwrap_or_else(|| panic!("a JSON-RPC result or error, got {body:?}"))
}

/// Send one JSON-RPC **request** on an open session and return the raw
/// response body. `params` is the method's params object (`json!({})` when it
/// takes none).
pub async fn call_method<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    session: &str,
    bearer: Option<&str>,
    method: &str,
    params: Value,
) -> String {
    call_method_with(client, path, session, bearer, &[], method, params).await
}

/// [`call_method`] carrying extra request headers.
pub async fn call_method_with<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    session: &str,
    bearer: Option<&str>,
    headers: &[(&str, &str)],
    method: &str,
    params: Value,
) -> String {
    let response = post_message_with(
        client,
        path,
        Some(session),
        bearer,
        headers,
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
/// response body.
pub async fn call_tool<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    tool: &str,
    bearer: Option<&str>,
) -> String {
    call_tool_with(client, path, tool, bearer, json!({})).await
}

/// [`call_tool`] with an `arguments` object.
pub async fn call_tool_with<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    tool: &str,
    bearer: Option<&str>,
    arguments: Value,
) -> String {
    call_tool_as(client, path, tool, bearer, &[], arguments).await
}

/// [`call_tool_with`] under caller-supplied headers, applied to the handshake
/// and the call alike.
pub async fn call_tool_as<E: Endpoint>(
    client: &TestClient<E>,
    path: &str,
    tool: &str,
    bearer: Option<&str>,
    headers: &[(&str, &str)],
    arguments: Value,
) -> String {
    let session = open_session_with(client, path, bearer, headers, json!({})).await;
    call_method_with(
        client,
        path,
        &session,
        bearer,
        headers,
        "tools/call",
        json!({ "name": tool, "arguments": arguments }),
    )
    .await
}
